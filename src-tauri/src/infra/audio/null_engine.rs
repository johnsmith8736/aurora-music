//! 无输出设备 / 测试替身音频引擎。
//!
//! # 为什么这是一个生产类型而不是 `#[cfg(test)]`
//!
//! 它承担两个职责：
//!
//! 1. **测试替身**：让播放推进、自动续播、截断检测等逻辑可以在
//!    **没有声卡、没有真实时间**的条件下被完整测试。这是整个播放
//!    逻辑可测性的基石。
//! 2. **无设备降级**：系统没有可用音频设备时，应用不应崩溃，
//!    而应能正常浏览资料库并给出明确提示。
//!
//! # 关于「不假装能播」
//!
//! `unavailable()` 构造的实例对任何播放请求都返回 `NoOutputDevice`，
//! **不会**假装播放成功然后毫无声音 —— 界面会如实显示错误。

use std::sync::Mutex;
use std::time::Duration;

use crate::domain::model::TrackId;
use crate::domain::ports::{AudioOutput, AudioSource, EngineEvent, TransportState};
use crate::domain::PlaybackError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 测试替身：模拟播放，时间由 `advance()` 驱动
    Scripted,
    /// 无设备降级：拒绝一切播放请求
    Unavailable,
}

struct Current {
    track_id: TrackId,
    declared_duration: Option<Duration>,
    end_reported: bool,
}

struct Inner {
    mode: Mode,
    current: Option<Current>,
    ever_loaded: bool,
    position: Duration,
    paused: bool,
    volume: f32,
    events: Vec<EngineEvent>,
    /// 测试注入：让下一次 load 失败，用于验证错误路径
    next_load_failure: Option<String>,
    /// 测试注入：让**每一次** load 都失败。
    ///
    /// 与一次性的 `next_load_failure` 相对：用于验证"整个队列都不可播"时
    /// 循环预算是否仍然有限 —— 一次性注入无法表达这种情形。
    persistent_load_failure: Option<String>,
}

pub struct NullAudioEngine {
    inner: Mutex<Inner>,
}

impl NullAudioEngine {
    /// 测试替身。位置完全由 `advance()` 驱动，行为确定。
    pub fn scripted() -> Self {
        Self {
            inner: Mutex::new(Inner {
                mode: Mode::Scripted,
                current: None,
                ever_loaded: false,
                position: Duration::ZERO,
                paused: false,
                volume: 1.0,
                events: Vec::new(),
                next_load_failure: None,
                persistent_load_failure: None,
            }),
        }
    }

    /// 无设备降级。任何播放请求都返回 `NoOutputDevice`。
    pub fn unavailable() -> Self {
        let mut e = Self::scripted();
        e.inner.get_mut().unwrap_or_else(|p| p.into_inner()).mode = Mode::Unavailable;
        e
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ─────────────────────────────────────────────────────────
    // 测试控制接口
    // ─────────────────────────────────────────────────────────

    /// 模拟时间流逝。仅在播放中且未暂停时推进位置。
    ///
    /// 位置达到声明时长时**自动产生** `TrackEnded`（正常播完）。
    pub fn advance(&self, delta: Duration) {
        let mut inner = self.lock();
        if inner.paused || inner.current.is_none() {
            return;
        }
        inner.position += delta;

        // 先把需要的数据复制出来，避免同时可变借用 inner 与 inner.current
        let pending_end = match inner.current.as_ref() {
            Some(c) if !c.end_reported => match c.declared_duration {
                Some(total) if inner.position >= total => Some((c.track_id, total)),
                _ => None,
            },
            _ => None,
        };

        if let Some((track_id, total)) = pending_end {
            // 自然结束：位置对齐到声明时长
            inner.position = total;
            if let Some(c) = inner.current.as_mut() {
                c.end_reported = true;
            }
            inner.events.push(EngineEvent::TrackEnded {
                track_id,
                position: total,
            });
        }
    }

    /// 在当前实际位置强制结束（模拟**截断文件**：声明与实际不符）。
    pub fn finish_current(&self) {
        let mut inner = self.lock();
        let ev = match inner.current.as_mut() {
            Some(c) if !c.end_reported => {
                c.end_reported = true;
                Some(EngineEvent::TrackEnded {
                    track_id: c.track_id,
                    position: inner.position,
                })
            }
            _ => None,
        };
        if let Some(e) = ev {
            inner.events.push(e);
        }
    }

    /// 让下一次 `load_and_play` 失败。
    pub fn fail_next_load(&self, reason: impl Into<String>) {
        self.lock().next_load_failure = Some(reason.into());
    }

    /// 让**每一次**后续 `load_and_play` 都失败，直到再次调用本方法覆盖。
    ///
    /// 用于构造"整个队列都不可播"的场景：验证自动续播的尝试次数上限
    /// 在环绕模式下依然有限，不会死循环。
    pub fn fail_every_load(&self, reason: impl Into<String>) {
        self.lock().persistent_load_failure = Some(reason.into());
    }

    /// 当前模拟位置，供断言使用。
    pub fn simulated_position(&self) -> Duration {
        self.lock().position
    }
}

impl AudioOutput for NullAudioEngine {
    fn load_and_play(&self, source: AudioSource) -> Result<(), PlaybackError> {
        let mut inner = self.lock();

        if inner.mode == Mode::Unavailable {
            return Err(PlaybackError::NoOutputDevice);
        }

        if let Some(reason) = inner.persistent_load_failure.clone() {
            return Err(PlaybackError::DecodeFailed {
                track_id: source.track_id,
                reason,
            });
        }

        if let Some(reason) = inner.next_load_failure.take() {
            return Err(PlaybackError::DecodeFailed {
                track_id: source.track_id,
                reason,
            });
        }

        inner.current = Some(Current {
            track_id: source.track_id,
            declared_duration: source.declared_duration,
            end_reported: false,
        });
        inner.ever_loaded = true;
        inner.position = Duration::ZERO;
        inner.paused = false;
        Ok(())
    }

    fn play(&self) -> Result<(), PlaybackError> {
        let mut inner = self.lock();
        if inner.current.is_none() {
            return Err(PlaybackError::NothingPlaying);
        }
        inner.paused = false;
        Ok(())
    }

    fn pause(&self) -> Result<(), PlaybackError> {
        let mut inner = self.lock();
        if inner.current.is_none() {
            return Err(PlaybackError::NothingPlaying);
        }
        inner.paused = true;
        Ok(())
    }

    fn stop(&self) -> Result<(), PlaybackError> {
        let mut inner = self.lock();
        inner.current = None;
        inner.position = Duration::ZERO;
        inner.paused = false;
        Ok(())
    }

    fn seek(&self, position: Duration) -> Result<(), PlaybackError> {
        let mut inner = self.lock();
        let cur = inner.current.as_ref().ok_or(PlaybackError::NothingPlaying)?;

        if let Some(total) = cur.declared_duration
            && position > total
        {
            return Err(PlaybackError::SeekOutOfRange {
                requested_ms: position.as_millis() as i64,
                duration_ms: total.as_millis() as i64,
            });
        }
        inner.position = position;
        // 跳转回到未结束状态，允许再次播完
        if let Some(c) = inner.current.as_mut() {
            c.end_reported = false;
        }
        Ok(())
    }

    fn set_volume(&self, volume: f32) -> Result<(), PlaybackError> {
        self.lock().volume = volume.clamp(0.0, 1.0);
        Ok(())
    }

    fn volume(&self) -> f32 {
        self.lock().volume
    }

    fn position(&self) -> Duration {
        self.lock().position
    }

    fn state(&self) -> TransportState {
        let inner = self.lock();
        if let Some(cur) = inner.current.as_ref() {
            // 已播完但尚未被服务层推进到下一首：不能再报 Playing，
            // 否则界面会在曲目结束后仍显示"播放中"。
            // （真实引擎也曾有同样的缺陷，由 tests/real_playback.rs 发现）
            if cur.end_reported {
                return TransportState::Stopped;
            }
            return if inner.paused {
                TransportState::Paused
            } else {
                TransportState::Playing
            };
        }
        if inner.ever_loaded {
            TransportState::Stopped
        } else {
            TransportState::Idle
        }
    }

    fn take_events(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.lock().events)
    }

    fn describe(&self) -> String {
        match self.lock().mode {
            Mode::Scripted => "null engine（测试替身，无声）".into(),
            Mode::Unavailable => "无可用音频输出设备".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: TrackId, ms: Option<u64>) -> AudioSource {
        AudioSource {
            track_id: id,
            path: format!("/music/{id}.mp3").into(),
            declared_duration: ms.map(Duration::from_millis),
        }
    }

    #[test]
    fn starts_idle_and_reports_nothing_playing() {
        let e = NullAudioEngine::scripted();
        assert_eq!(e.state(), TransportState::Idle);
        assert_eq!(e.position(), Duration::ZERO);
        assert!(matches!(
            e.play().unwrap_err(),
            PlaybackError::NothingPlaying
        ));
    }

    #[test]
    fn position_advances_only_while_playing() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(10_000))).unwrap();
        assert_eq!(e.state(), TransportState::Playing);

        e.advance(Duration::from_millis(1000));
        assert_eq!(e.position(), Duration::from_millis(1000));

        e.pause().unwrap();
        e.advance(Duration::from_millis(1000));
        assert_eq!(e.position(), Duration::from_millis(1000), "暂停时不应推进");

        e.play().unwrap();
        e.advance(Duration::from_millis(500));
        assert_eq!(e.position(), Duration::from_millis(1500));
    }

    #[test]
    fn reaching_declared_duration_emits_track_ended_once() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(42, Some(1000))).unwrap();

        e.advance(Duration::from_millis(1000));
        let evs = e.take_events();
        assert_eq!(evs.len(), 1);
        assert_eq!(
            evs[0],
            EngineEvent::TrackEnded {
                track_id: 42,
                position: Duration::from_millis(1000)
            }
        );

        // 再次推进不得重复上报
        e.advance(Duration::from_millis(500));
        assert!(e.take_events().is_empty(), "结束事件只能上报一次");
    }

    #[test]
    fn finish_current_emits_at_actual_position_not_declared() {
        // 这正是「截断文件」场景：声明 3000 ms，实际只播到 1900 ms
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(7, Some(3000))).unwrap();
        e.advance(Duration::from_millis(1900));
        e.finish_current();

        let evs = e.take_events();
        assert_eq!(
            evs[0],
            EngineEvent::TrackEnded {
                track_id: 7,
                position: Duration::from_millis(1900)
            },
            "结束位置必须是实际位置，而非声明时长"
        );
    }

    /// 回归测试：曲目自然结束后，状态**不得**仍为 Playing。
    ///
    /// 此前的缺陷：`state()` 只看 `current.is_some()`，而播完后只置了
    /// `end_reported` 没清 `current`，导致界面在曲目结束后仍显示"播放中"。
    /// 真实引擎上的同一缺陷由 `tests/real_playback.rs` 在真实声卡上发现。
    #[test]
    fn state_is_stopped_after_natural_end() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(1000))).unwrap();
        assert_eq!(e.state(), TransportState::Playing);

        e.advance(Duration::from_millis(1000));
        e.take_events(); // 消费结束事件，模拟服务层已处理

        assert_eq!(
            e.state(),
            TransportState::Stopped,
            "播完后必须报 Stopped 而不是 Playing"
        );
    }

    /// 同上，但针对「状态查询先于事件消费」的顺序。
    #[test]
    fn state_is_correct_even_before_events_are_drained() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(1000))).unwrap();
        e.advance(Duration::from_millis(1000));

        // 不调用 take_events，直接查状态
        assert_eq!(e.state(), TransportState::Stopped);

        // 事件仍然保留，不能因为查了状态就丢掉
        assert_eq!(
            e.take_events().len(),
            1,
            "查询状态不得吞掉待处理的结束事件"
        );
    }

    #[test]
    fn unknown_duration_never_auto_ends() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, None)).unwrap();
        e.advance(Duration::from_secs(3600));
        assert!(e.take_events().is_empty(), "无声明时长时不应自动判定结束");
        assert_eq!(e.state(), TransportState::Playing);
    }

    #[test]
    fn stop_clears_position_and_source() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(5000))).unwrap();
        e.advance(Duration::from_millis(1000));
        e.stop().unwrap();

        assert_eq!(e.position(), Duration::ZERO);
        assert_eq!(e.state(), TransportState::Stopped, "停止后应是 Stopped 而非 Idle");
        assert!(matches!(
            e.play().unwrap_err(),
            PlaybackError::NothingPlaying
        ));
    }

    #[test]
    fn seek_validates_against_declared_duration() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(1000))).unwrap();

        assert!(e.seek(Duration::from_millis(500)).is_ok());
        assert_eq!(e.position(), Duration::from_millis(500));

        let err = e.seek(Duration::from_millis(2000)).unwrap_err();
        match err {
            PlaybackError::SeekOutOfRange {
                requested_ms,
                duration_ms,
            } => {
                assert_eq!(requested_ms, 2000);
                assert_eq!(duration_ms, 1000);
            }
            other => panic!("期望 SeekOutOfRange，实际 {other}"),
        }
    }

    #[test]
    fn seek_back_before_end_allows_ending_again() {
        let e = NullAudioEngine::scripted();
        e.load_and_play(src(1, Some(1000))).unwrap();
        e.advance(Duration::from_millis(1000));
        assert_eq!(e.take_events().len(), 1);

        // 回到中间再播到结尾，应能再次上报结束
        e.seek(Duration::from_millis(200)).unwrap();
        e.advance(Duration::from_millis(800));
        assert_eq!(e.take_events().len(), 1, "跳转后应能再次正常结束");
    }

    #[test]
    fn unavailable_mode_honestly_refuses_to_play() {
        let e = NullAudioEngine::unavailable();
        let err = e.load_and_play(src(1, Some(1000))).unwrap_err();
        assert!(
            matches!(err, PlaybackError::NoOutputDevice),
            "无设备时必须如实报错，而不是假装播放成功"
        );
        assert!(e.describe().contains("无可用音频输出设备"));
    }

    #[test]
    fn injected_load_failure_surfaces_once() {
        let e = NullAudioEngine::scripted();
        e.fail_next_load("模拟解码失败");

        let err = e.load_and_play(src(3, Some(1000))).unwrap_err();
        assert!(matches!(err, PlaybackError::DecodeFailed { track_id: 3, .. }));

        // 注入只生效一次
        assert!(e.load_and_play(src(4, Some(1000))).is_ok());
    }
}
