//! 队列相关命令：播放模式。
//!
//! 命名遵循 `docs/architecture.md` 已规定的 `queue_set_*`，
//! 与 `playback_*` 分开 —— 队列操作是独立的命令域，
//! M2 的队列侧栏还会往这里加命令。
//!
//! 模式状态由 Rust 持有（`roadmap.md` 的"Rust 为唯一真相源"），
//! 前端只是它的镜像；因此这里是 setter，读回走 `playback_get_state`。

use tauri::State;

use crate::app::AppState;
use crate::domain::queue::RepeatMode;
use crate::domain::ErrorPayload;

/// 设置循环模式。前端传 `{ mode: "off" | "all" | "one" }`。
#[tauri::command]
pub async fn queue_set_repeat(
    state: State<'_, AppState>,
    mode: RepeatMode,
) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || {
        playback.set_repeat(mode);
        Ok(())
    })
    .await
}

/// 开启或关闭随机播放。
///
/// 开启时服务层生成一份固定置换（FR-3.5）；关闭时恢复队列原顺序。
/// 两种情况都**不打断正在播放的曲目**。
#[tauri::command]
pub async fn queue_set_shuffle(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || {
        playback.set_shuffle(enabled);
        Ok(())
    })
    .await
}
