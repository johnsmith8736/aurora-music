//! **真实音频输出**的端到端验证。
//!
//! 与 `ipc_commands.rs` 的区别：那里的播放命令跑在 `NullAudioEngine` 上
//! （无声、确定性）。这里的测试使用**真正的 rodio + cpal 引擎**，
//! 声音会真实送到系统音频设备。
//!
//! # 为什么必须有这一层
//!
//! 无声替身能证明「逻辑正确」，但不能证明「真的出声」。历史上最常见
//! 的失败模式恰恰是：逻辑全绿、界面正常、**但没有任何声音**。
//!
//! # 在无声卡环境
//!
//! 打不开设备时测试**跳过**而不是失败 —— CI 容器通常没有音频设备。

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurora_music_lib::domain::ports::{AudioOutput, AudioSource, EngineEvent, TransportState};
use aurora_music_lib::infra::audio::RodioAudioEngine;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// 打开真实引擎；无设备时返回 None（调用方跳过测试）。
fn real_engine() -> Option<RodioAudioEngine> {
    match RodioAudioEngine::open_default() {
        Ok(e) => Some(e),
        Err(err) => {
            eprintln!("跳过真实播放测试：本机无可用音频输出设备（{err}）");
            None
        }
    }
}

/// 轮询直到位置推进或超时。
fn wait_for_position_advance(engine: &RodioAudioEngine, timeout: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let pos = engine.position();
        if pos > Duration::ZERO {
            return Some(pos);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// 轮询直到引擎上报 TrackEnded 或超时。
fn wait_for_end(engine: &RodioAudioEngine, timeout: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        for ev in engine.take_events() {
            if let EngineEvent::TrackEnded { position, .. } = ev {
                return Some(position);
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    None
}

#[test]
fn real_engine_plays_a_file_to_completion() {
    let Some(engine) = real_engine() else { return };

    // 音量调低：这是自动化测试，不应制造噪音
    engine.set_volume(0.05).unwrap();

    let path = fixture("tagged.flac");
    assert!(path.exists(), "缺少夹具: {path:?}");

    engine
        .load_and_play(AudioSource {
            track_id: 1,
            path: path.clone(),
            declared_duration: Some(Duration::from_millis(1000)),
        })
        .expect("加载夹具应成功");

    assert_eq!(engine.state(), TransportState::Playing);

    // ── 证据 1：位置真的在推进（说明音频时钟在走）──────────────
    let first_pos = wait_for_position_advance(&engine, Duration::from_secs(3));
    assert!(
        first_pos.is_some(),
        "3 秒内播放位置没有任何推进 —— 音频未真正输出"
    );
    eprintln!("  位置已推进到 {:?}", first_pos.unwrap());

    // ── 证据 2：曲目自然结束时位置接近声明时长 ─────────────────
    let end_pos = wait_for_end(&engine, Duration::from_secs(5))
        .expect("5 秒内未收到 TrackEnded —— 曲目未正常结束");

    let ms = end_pos.as_millis() as i64;
    assert!(
        (900..=1100).contains(&ms),
        "结束时位置应接近 1000 ms（实测 {ms} ms）"
    );
    eprintln!("  自然结束位置: {ms} ms");

    // ── 证据 3：结束后状态正确 ─────────────────────────────────
    assert_ne!(
        engine.state(),
        TransportState::Playing,
        "播放结束后不应仍处于 Playing"
    );
}

#[test]
fn real_engine_rejects_corrupt_file_at_load() {
    let Some(engine) = real_engine() else { return };

    // 纯文本伪装成 mp3
    let bad = fixture("not_audio.mp3");
    if bad.exists() {
        let r = engine.load_and_play(AudioSource {
            track_id: 1,
            path: bad,
            declared_duration: None,
        });
        assert!(r.is_err(), "伪装文件必须在加载阶段被拒绝");
    }

    // 容器结构损坏
    let truncated = fixture("truncated.flac");
    if truncated.exists() {
        let r = engine.load_and_play(AudioSource {
            track_id: 2,
            path: truncated,
            declared_duration: Some(Duration::from_millis(1000)),
        });
        assert!(r.is_err(), "结构损坏的文件必须在加载阶段被拒绝");
    }
}

#[test]
fn real_engine_reports_early_end_for_truncated_audio() {
    // 这是「被接受但提前结束」的损坏模式 —— 扫描阶段发现不了，
    // 只能靠播放结束时的「实际 vs 声明」比对（见 architecture.md §10.2 F-4）
    let Some(engine) = real_engine() else { return };
    engine.set_volume(0.05).unwrap();

    let path = fixture("decodes_short.flac");
    assert!(path.exists(), "缺少夹具: {path:?}");

    // 该文件容器声明 3000 ms，但音频数据只有约 64%
    engine
        .load_and_play(AudioSource {
            track_id: 9,
            path,
            declared_duration: Some(Duration::from_millis(3000)),
        })
        .expect("截断文件应能被解码器接受（这正是危险之处）");

    let end_pos = wait_for_end(&engine, Duration::from_secs(8))
        .expect("应能收到结束事件");

    let ms = end_pos.as_millis() as i64;
    let ratio = ms as f64 / 3000.0;
    eprintln!("  声明 3000 ms，实际结束于 {ms} ms（{:.0}%）", ratio * 100.0);

    assert!(
        ratio < 0.90,
        "截断文件应在明显短于声明时长处结束，实际比值 {ratio:.2} —— \
         若该断言失败，说明截断检测的前提不再成立"
    );
}

#[test]
fn real_engine_survives_rapid_track_switching() {
    let Some(engine) = real_engine() else { return };
    engine.set_volume(0.0).unwrap(); // 本测试只验证稳定性，静音

    let files = ["tagged.flac", "tagged.mp3", "07 - Untagged Track.wav"];
    let mut loaded = 0;

    for round in 0..3 {
        for (i, name) in files.iter().enumerate() {
            let path = fixture(name);
            if !path.exists() {
                continue;
            }
            engine
                .load_and_play(AudioSource {
                    track_id: (round * 10 + i) as i64,
                    path,
                    declared_duration: Some(Duration::from_millis(1000)),
                })
                .unwrap_or_else(|e| panic!("第 {round} 轮加载 {name} 失败: {e}"));
            loaded += 1;
            std::thread::sleep(Duration::from_millis(60));
        }
    }

    assert_eq!(loaded, 9, "应完成 9 次快速切换");
    engine.take_events(); // 清空累积事件，不应 panic
    let _ = engine.state();
}

#[test]
fn real_engine_seek_moves_position() {
    let Some(engine) = real_engine() else { return };
    engine.set_volume(0.0).unwrap();

    engine
        .load_and_play(AudioSource {
            track_id: 1,
            path: fixture("tagged.flac"),
            declared_duration: Some(Duration::from_millis(1000)),
        })
        .unwrap();

    engine
        .seek(Duration::from_millis(700))
        .expect("跳转到 700 ms 应成功");

    // 跳转后位置应立即接近目标
    let pos = engine.position();
    assert!(
        pos >= Duration::from_millis(600),
        "跳转后位置应接近 700 ms，实际 {pos:?}"
    );

    // 超出范围必须报错而非静默钳制
    let err = engine.seek(Duration::from_millis(9999)).unwrap_err();
    assert!(
        matches!(
            err,
            aurora_music_lib::domain::PlaybackError::SeekOutOfRange { .. }
        ),
        "越界跳转必须报 SeekOutOfRange，实际 {err:?}"
    );
}

#[test]
fn engine_is_shareable_across_threads() {
    // 编译期已由 Send+Sync 约束保证，这里再验证运行期确实可用：
    // 播放推进线程与查询线程分离，正是生产代码的用法。
    let Some(engine) = real_engine() else { return };
    engine.set_volume(0.0).unwrap();

    let shared = Arc::new(engine);
    let reader = shared.clone();

    let handle = std::thread::spawn(move || {
        let mut max_pos = Duration::ZERO;
        for _ in 0..40 {
            max_pos = max_pos.max(reader.position());
            std::thread::sleep(Duration::from_millis(25));
        }
        max_pos
    });

    shared
        .load_and_play(AudioSource {
            track_id: 1,
            path: fixture("tagged.flac"),
            declared_duration: Some(Duration::from_millis(1000)),
        })
        .unwrap();

    let max_pos = handle.join().expect("读取线程不应 panic");
    assert!(
        max_pos > Duration::ZERO,
        "跨线程读取位置应能观察到推进，实际 {max_pos:?}"
    );
}
