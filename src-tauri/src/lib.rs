//! Aurora Music —— 面向 Linux 桌面的本地音乐播放器。
//!
//! 分层（docs/architecture.md §1）：
//! ```text
//! commands/  薄适配器，仅做参数校验与转接
//!     ↓
//! app/       应用服务：编排、事务、播放推进
//!     ↓ 只依赖 trait / 纯逻辑
//! domain/    领域层：纯逻辑，零 I/O
//!     ↑ 实现
//! infra/     基础设施：唯一触碰外部世界的地方
//! ```

pub mod app;
pub mod commands;
pub mod domain;
pub mod infra;

use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, Manager};

use app::{AppState, PlaybackService, PlaybackSnapshot};

/// 播放状态轮询周期。
///
/// 200 ms ≈ 5 Hz：足够让界面插值出平滑进度条，又不会给 IPC 造成负担。
/// 架构文档要求位置更新不低于 4 Hz。
const PLAYBACK_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// 后台轮询线程：消费引擎事件、推进队列、向界面广播状态。
///
/// 用 `std::thread` 而非 tokio 任务：这里就是一个固定间隔的轻量循环，
/// 不引入额外的异步依赖。（音频本身由 rodio/cpal 的回调线程输出，
/// 本线程只做状态搬运。）
fn spawn_playback_pump(app_handle: tauri::AppHandle, playback: Arc<PlaybackService>) {
    std::thread::Builder::new()
        .name("playback-pump".into())
        .spawn(move || {
            let mut last_sent: Option<PlaybackSnapshot> = None;

            loop {
                std::thread::sleep(PLAYBACK_POLL_INTERVAL);

                // 推进：处理曲目结束、截断检测、自动续播
                let events = playback.tick();
                for event in &events {
                    let _ = app_handle.emit("playback:event", event);
                }

                // 状态变化才广播 —— 空闲时不会产生任何 IPC 流量
                let snapshot = playback.snapshot();
                if last_sent.as_ref() != Some(&snapshot) {
                    let _ = app_handle.emit("playback:state", &snapshot);
                    last_sent = Some(snapshot);
                }
            }
        })
        .expect("无法创建播放轮询线程");
}

/// 初始化数据库、音频引擎并装配应用。
fn setup_app(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = infra::db::default_db_path()?;
    let pool = infra::db::open(&db_path)?;

    // 打不开真实音频设备时会降级到静默引擎，而不是让应用启动失败
    let engine = infra::audio::open_best_engine();
    let clock: Arc<dyn domain::ports::Clock> = Arc::new(infra::time::SystemClock);
    let random: Arc<dyn domain::ports::RandomSource> = Arc::new(infra::random::SystemRandom);
    let playback = Arc::new(PlaybackService::new(engine, clock, random));

    spawn_playback_pump(app.clone(), playback.clone());

    app.manage(AppState::new(pool, playback));
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            setup_app(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 资料库
            commands::library::library_add_root,
            commands::library::library_list_roots,
            commands::library::library_remove_root,
            commands::library::library_scan_root,
            commands::library::library_list_tracks,
            commands::library::library_get_track,
            commands::library::library_stats,
            // 播放
            commands::playback::playback_play_tracks,
            commands::playback::playback_toggle,
            commands::playback::playback_stop,
            commands::playback::playback_next,
            commands::playback::playback_previous,
            commands::playback::playback_seek,
            commands::playback::playback_set_volume,
            commands::playback::playback_get_state,
            // 队列（播放模式）
            commands::queue::queue_set_repeat,
            commands::queue::queue_set_shuffle,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
