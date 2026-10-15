//! 播放应用服务：编排队列、引擎与错误处理。
//!
//! # 设计要点
//!
//! - **`tick()` 是唯一的推进入口**。它消费引擎事件、检测截断、自动续播，
//!   并返回需要通知前端的事件。这样所有播放推进逻辑都能用
//!   `NullAudioEngine` 在无声音、无真实时间的条件下确定性地测试。
//! - 服务不依赖 `tauri`。命令层与轮询任务负责把 `tick()` 的结果转成 IPC。
//! - 引擎的加载失败**不中断队列**（FR-2.6）：记录错误后跳到下一首。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::domain::model::TrackId;
use crate::domain::ports::{
    AudioOutput, AudioSource, Clock, EngineEvent, RandomSource, TransportState,
};
use crate::domain::queue::{PlayQueue, QueueEntry, RepeatMode};
use crate::domain::{AppError, PlaybackError};

/// 判定截断的最小声明时长。短于该值的曲目不参与判定 ——
/// 编码器 padding 在短文件上会显著影响比值（实测 1 秒 MP3 报 1152 ms）。
const TRUNCATION_MIN_DECLARED: Duration = Duration::from_millis(2000);
/// 实际/声明 比值低于此值判定为截断。实测截断样本为 64%。
const TRUNCATION_RATIO: f64 = 0.90;

/// 播放过程中产生的、需要通知前端的事件。
///
/// ⚠️ `rename_all` 在枚举上**只重命名变体名**，不会重命名变体内的字段。
/// 必须额外加 `rename_all_fields`，否则会序列化成 `{"kind":"trackStarted","track_id":7}`
/// 这种字段名不一致的载荷（已由 `tests/ipc_contract.rs` 锁定）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlaybackEvent {
    TrackStarted {
        track_id: TrackId,
    },
    TrackEnded {
        track_id: TrackId,
    },
    /// 曲目出问题并被跳过（解码失败 / 文件截断）。
    TrackFailed {
        track_id: TrackId,
        code: String,
        message: String,
    },
    /// 整个队列播放完毕。
    QueueFinished,
    DeviceLost {
        reason: String,
    },
}

/// 最近一次曲目级失败，供界面展示。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFailure {
    pub track_id: TrackId,
    pub code: String,
    pub message: String,
}

/// 播放状态快照。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSnapshot {
    pub status: TransportState,
    pub track_id: Option<TrackId>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub position_ms: i64,
    pub volume: f32,
    pub queue_length: usize,
    pub queue_index: Option<usize>,
    pub has_next: bool,
    pub has_previous: bool,
    /// 循环模式。序列化为 `"off"` / `"all"` / `"one"`。
    pub repeat: RepeatMode,
    /// 随机播放开关。置换本身（`order`）不跨 IPC —— 前端只需要这一个布尔量。
    pub shuffle: bool,
    pub engine: String,
    pub last_failure: Option<TrackFailure>,
}

struct Inner {
    queue: PlayQueue,
    volume: f32,
    last_failure: Option<TrackFailure>,
}

pub struct PlaybackService {
    engine: Arc<dyn AudioOutput>,
    #[allow(dead_code)] // 预留：播放历史需要时间戳，属下一个里程碑
    clock: Arc<dyn Clock>,
    /// 洗牌种子来源。见 `domain/ports.rs::RandomSource`。
    random: Arc<dyn RandomSource>,
    inner: Mutex<Inner>,
}

impl PlaybackService {
    pub fn new(
        engine: Arc<dyn AudioOutput>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
    ) -> Self {
        Self {
            engine,
            clock,
            random,
            inner: Mutex::new(Inner {
                queue: PlayQueue::new(),
                volume: 1.0,
                last_failure: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ─────────────────────────────────────────────────────────
    // 命令
    // ─────────────────────────────────────────────────────────

    /// 用给定曲目整体替换队列，并从 `start_index` 开始播放。
    pub fn play_tracks(
        &self,
        entries: Vec<QueueEntry>,
        start_index: usize,
    ) -> Result<PlaybackEvent, AppError> {
        if entries.is_empty() {
            return Err(AppError::Playback(PlaybackError::EmptyQueue));
        }
        if start_index >= entries.len() {
            return Err(AppError::InvalidArgument {
                field: "startIndex",
                detail: format!("{start_index} 超出队列长度 {}", entries.len()),
            });
        }

        {
            let mut inner = self.lock();
            if inner
                .queue
                .replace(entries, start_index)
                .is_none()
            {
                return Err(AppError::Playback(PlaybackError::EmptyQueue));
            }
            inner.last_failure = None;
        }

        let entry = self
            .lock()
            .queue
            .current()
            .cloned()
            .ok_or(AppError::Playback(PlaybackError::EmptyQueue))?;

        self.load(&entry)?;
        Ok(PlaybackEvent::TrackStarted {
            track_id: entry.track_id,
        })
    }

    /// 播放 / 暂停切换。
    pub fn toggle(&self) -> Result<(), AppError> {
        let has_current = self.lock().queue.current().is_some();
        if !has_current {
            return Err(AppError::Playback(PlaybackError::NothingPlaying));
        }

        match self.engine.state() {
            TransportState::Playing => self.engine.pause()?,
            _ => self.engine.play()?,
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<(), AppError> {
        self.engine.stop()?;
        let mut inner = self.lock();
        inner.queue.clear();
        inner.last_failure = None;
        Ok(())
    }

    /// 手动下一首。
    pub fn next(&self) -> Result<Option<PlaybackEvent>, AppError> {
        let entry = {
            let mut inner = self.lock();
            inner.queue.advance().cloned()
        };
        let Some(entry) = entry else {
            // 已在末曲：停止播放但不报错
            self.engine.stop()?;
            let mut inner = self.lock();
            inner.queue.clear();
            return Ok(None);
        };

        self.load(&entry)?;
        Ok(Some(PlaybackEvent::TrackStarted {
            track_id: entry.track_id,
        }))
    }

    /// 手动上一首。
    pub fn previous(&self) -> Result<Option<PlaybackEvent>, AppError> {
        let entry = {
            let mut inner = self.lock();
            inner.queue.retreat().cloned()
        };
        let Some(entry) = entry else {
            return Ok(None); // 已在首曲，无操作
        };

        self.load(&entry)?;
        Ok(Some(PlaybackEvent::TrackStarted {
            track_id: entry.track_id,
        }))
    }

    /// 跳到队列中的指定下标。
    pub fn jump_to(&self, index: usize) -> Result<PlaybackEvent, AppError> {
        let entry = {
            let mut inner = self.lock();
            inner.queue.jump(index).cloned()
        };
        let entry = entry.ok_or(AppError::InvalidArgument {
            field: "index",
            detail: "超出队列范围".into(),
        })?;

        self.load(&entry)?;
        Ok(PlaybackEvent::TrackStarted {
            track_id: entry.track_id,
        })
    }

    pub fn seek(&self, position_ms: i64) -> Result<(), AppError> {
        if position_ms < 0 {
            return Err(AppError::InvalidArgument {
                field: "positionMs",
                detail: "不能为负数".into(),
            });
        }
        self.engine.seek(Duration::from_millis(position_ms as u64))?;
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), AppError> {
        if !volume.is_finite() {
            return Err(AppError::InvalidArgument {
                field: "volume",
                detail: "必须是有限数值".into(),
            });
        }
        self.engine.set_volume(volume)?;
        self.lock().volume = volume.clamp(0.0, 1.0);
        Ok(())
    }

    /// 设置循环模式。
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.lock().queue.set_repeat(mode);
    }

    /// 开启或关闭随机播放。
    ///
    /// 种子取自 `RandomSource`。关闭时不消耗种子 —— 开关切换不该影响
    /// 其它依赖随机序列的行为（目前没有，但保持这个性质没有成本）。
    pub fn set_shuffle(&self, enabled: bool) {
        let seed = if enabled { self.random.seed() } else { 0 };
        self.lock().queue.set_shuffle(enabled, seed);
    }

    // ─────────────────────────────────────────────────────────
    // 推进与状态
    // ─────────────────────────────────────────────────────────

    /// 处理引擎事件、检测截断、自动续播。
    ///
    /// 由轮询任务周期性调用。**这是唯一的播放推进入口**，
    /// 因此也是测试的主要着力点。
    pub fn tick(&self) -> Vec<PlaybackEvent> {
        let engine_events = self.engine.take_events();
        let mut out = Vec::new();

        for ev in engine_events {
            match ev {
                EngineEvent::TrackEnded { track_id, position } => {
                    if let Some(failure) = self.check_truncation(track_id, position) {
                        // 失败路径**一律前进**，绝不重播 —— 理由见 `replay_current`
                        out.push(PlaybackEvent::TrackFailed {
                            track_id,
                            code: failure.code.clone(),
                            message: failure.message.clone(),
                        });
                        self.lock().last_failure = Some(failure);
                        out.extend(self.auto_advance());
                        continue;
                    }
                    out.push(PlaybackEvent::TrackEnded { track_id });

                    if self.repeat_one_active() {
                        out.extend(self.replay_current());
                    } else {
                        out.extend(self.auto_advance());
                    }
                }
                EngineEvent::LoadFailed { track_id, reason } => {
                    let failure = TrackFailure {
                        track_id,
                        code: "PLAYBACK_DECODE_FAILED".into(),
                        message: "无法播放该文件，格式可能不受支持或文件已损坏。".into(),
                    };
                    out.push(PlaybackEvent::TrackFailed {
                        track_id,
                        code: failure.code.clone(),
                        message: failure.message.clone(),
                    });
                    self.lock().last_failure = Some(failure);
                    let _ = reason; // 细节不进入用户可见消息（NFR-4.5）
                    out.extend(self.auto_advance());
                }
                EngineEvent::DeviceLost { reason } => {
                    out.push(PlaybackEvent::DeviceLost { reason });
                }
            }
        }

        out
    }

    /// 截断检测：结束时实际位置显著短于声明时长 ⇒ 判定文件损坏。
    ///
    /// 对应架构文档 §10.2 F-4：`Decoder` 的 `Item` 是 `Sample` 而非
    /// `Result`，解码错误不会被暴露，迭代器只是安静地结束。
    /// 因此**必须**用「实际 vs 声明」比对来发现这类损坏。
    fn check_truncation(&self, track_id: TrackId, actual: Duration) -> Option<TrackFailure> {
        let declared_ms = {
            let inner = self.lock();
            inner.queue.current()?.duration_ms
        }?;
        if declared_ms <= 0 {
            return None;
        }
        let declared = Duration::from_millis(declared_ms as u64);

        if !is_truncated(actual, declared) {
            return None;
        }

        Some(TrackFailure {
            track_id,
            code: "PLAYBACK_TRUNCATED_FILE".into(),
            message: "该文件在播放中途意外结束，可能已损坏，已跳到下一首。".into(),
        })
    }

    /// 自动前进到下一首**可播放**的曲目。
    ///
    /// 连续失败时最多尝试整个队列长度，避免全损坏队列造成无限循环。
    fn auto_advance(&self) -> Vec<PlaybackEvent> {
        let mut out = Vec::new();
        let attempts = self.lock().queue.len();

        for _ in 0..attempts {
            let entry = {
                let mut inner = self.lock();
                inner.queue.advance().cloned()
            };
            let Some(entry) = entry else {
                // 队列播完
                let _ = self.engine.stop();
                out.push(PlaybackEvent::QueueFinished);
                return out;
            };

            match self.load(&entry) {
                Ok(()) => {
                    out.push(PlaybackEvent::TrackStarted {
                        track_id: entry.track_id,
                    });
                    return out;
                }
                Err(err) => {
                    let failure = TrackFailure {
                        track_id: entry.track_id,
                        code: err.code().to_string(),
                        message: err.user_message(),
                    };
                    out.push(PlaybackEvent::TrackFailed {
                        track_id: failure.track_id,
                        code: failure.code.clone(),
                        message: failure.message.clone(),
                    });
                    self.lock().last_failure = Some(failure);
                    continue;
                }
            }
        }

        // 尝试了整队都无法播放
        let _ = self.engine.stop();
        out.push(PlaybackEvent::QueueFinished);
        out
    }

    /// 单曲循环是否生效。
    ///
    /// 注意 `RepeatMode::One` **不影响**队列的 `advance()`/`retreat()`：
    /// 手动点"下一首"在任何模式下都应前进，单曲循环只作用于自然结束后的续播。
    fn repeat_one_active(&self) -> bool {
        self.lock().queue.repeat() == RepeatMode::One
    }

    /// 重播当前曲目（单曲循环）。
    ///
    /// ⚠️ **只允许在自然结束时调用。** 任何失败路径都必须走 `auto_advance()`：
    /// 一个被判定为截断的文件每次重播都会再次提前结束，若失败路径也重播，
    /// 就会无限循环播放同一段损坏音频。`tick()` 里两条分支的划分即为此。
    fn replay_current(&self) -> Vec<PlaybackEvent> {
        let entry = {
            let inner = self.lock();
            inner.queue.current().cloned()
        };
        let Some(entry) = entry else {
            // 队列空了（理论上不该发生），退回常规续播
            return self.auto_advance();
        };

        match self.load(&entry) {
            Ok(()) => vec![PlaybackEvent::TrackStarted {
                track_id: entry.track_id,
            }],
            Err(err) => {
                let failure = TrackFailure {
                    track_id: entry.track_id,
                    code: err.code().to_string(),
                    message: err.user_message(),
                };
                self.lock().last_failure = Some(failure.clone());
                let mut out = vec![PlaybackEvent::TrackFailed {
                    track_id: failure.track_id,
                    code: failure.code,
                    message: failure.message,
                }];
                // 连当前这首都放不出来，只能往下走
                out.extend(self.auto_advance());
                out
            }
        }
    }

    fn load(&self, entry: &QueueEntry) -> Result<(), AppError> {
        let source = AudioSource {
            track_id: entry.track_id,
            path: entry.path.clone(),
            declared_duration: entry
                .duration_ms
                .filter(|d| *d > 0)
                .map(|d| Duration::from_millis(d as u64)),
        };
        self.engine.load_and_play(source)?;
        Ok(())
    }

    /// 当前状态快照。
    pub fn snapshot(&self) -> PlaybackSnapshot {
        let inner = self.lock();
        let cur = inner.queue.current();
        let duration_ms = cur.and_then(|c| c.duration_ms);

        // 位置以声明时长封顶，避免界面出现 100.3%
        let position = self.engine.position().as_millis() as i64;
        let position_ms = match duration_ms {
            Some(d) if d > 0 => position.min(d),
            _ => position,
        };

        PlaybackSnapshot {
            status: self.engine.state(),
            track_id: cur.map(|c| c.track_id),
            title: cur.map(|c| c.title.clone()),
            artist: cur.and_then(|c| c.artist.clone()),
            album: cur.and_then(|c| c.album.clone()),
            duration_ms,
            position_ms,
            volume: inner.volume,
            queue_length: inner.queue.len(),
            queue_index: inner.queue.current_index(),
            has_next: inner.queue.has_next(),
            has_previous: inner.queue.has_previous(),
            repeat: inner.queue.repeat(),
            shuffle: inner.queue.is_shuffled(),
            engine: self.engine.describe(),
            last_failure: inner.last_failure.clone(),
        }
    }

    /// 队列内容（供界面渲染）。
    pub fn queue_entries(&self) -> Vec<QueueEntry> {
        self.lock().queue.items().to_vec()
    }
}

/// 判定「被接受但提前结束」的截断文件。
///
/// 两个阈值都是实测得出的，不是拍脑袋：
/// - `TRUNCATION_MIN_DECLARED` = 2 s：短文件的编码器 padding 会显著影响比值
///   （实测 1 秒 MP3 报 1152 ms）
/// - `TRUNCATION_RATIO` = 0.90：实测截断样本为 64%，正常文件为 100%
fn is_truncated(actual: Duration, declared: Duration) -> bool {
    if declared < TRUNCATION_MIN_DECLARED {
        return false;
    }
    actual.as_secs_f64() < declared.as_secs_f64() * TRUNCATION_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::audio::NullAudioEngine;
    use crate::infra::random::FakeRandom;
    use crate::infra::time::FakeClock;
    use std::path::PathBuf;

    fn entry(id: TrackId, ms: Option<i64>) -> QueueEntry {
        QueueEntry {
            track_id: id,
            title: format!("Track {id}"),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            path: PathBuf::from(format!("/music/{id}.flac")),
            duration_ms: ms,
        }
    }

    struct Harness {
        service: PlaybackService,
        engine: Arc<NullAudioEngine>,
    }

    fn harness() -> Harness {
        let engine = Arc::new(NullAudioEngine::scripted());
        let service = PlaybackService::new(
            engine.clone(),
            Arc::new(FakeClock::new(1_000_000)),
            Arc::new(FakeRandom::new()),
        );
        Harness { service, engine }
    }

    #[test]
    fn playing_tracks_sets_queue_and_starts_first_item() {
        let h = harness();
        let entries = vec![entry(1, Some(5000)), entry(2, Some(5000))];
        let ev = h.service.play_tracks(entries, 0).unwrap();
        assert_eq!(ev, PlaybackEvent::TrackStarted { track_id: 1 });

        let snap = h.service.snapshot();
        assert_eq!(snap.status, TransportState::Playing);
        assert_eq!(snap.track_id, Some(1));
        assert_eq!(snap.title.as_deref(), Some("Track 1"));
        assert_eq!(snap.queue_length, 2);
        assert_eq!(snap.queue_index, Some(0));
        assert!(snap.has_next);
        assert!(!snap.has_previous);
    }

    #[test]
    fn starting_index_is_honoured() {
        let h = harness();
        let entries = vec![entry(1, Some(1000)), entry(2, Some(1000)), entry(3, Some(1000))];
        let ev = h.service.play_tracks(entries, 2).unwrap();
        assert_eq!(ev, PlaybackEvent::TrackStarted { track_id: 3 });
        let snap = h.service.snapshot();
        assert_eq!(snap.queue_index, Some(2));
        assert!(snap.has_previous);
        assert!(!snap.has_next);
    }

    #[test]
    fn empty_queue_and_bad_index_are_rejected() {
        let h = harness();
        assert!(matches!(
            h.service.play_tracks(vec![], 0).unwrap_err(),
            AppError::Playback(PlaybackError::EmptyQueue)
        ));
        assert!(matches!(
            h.service.play_tracks(vec![entry(1, Some(1000))], 5).unwrap_err(),
            AppError::InvalidArgument { .. }
        ));
    }

    #[test]
    fn track_end_automatically_advances_to_next() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();

        h.engine.advance(Duration::from_millis(1000));
        let events = h.service.tick();

        assert_eq!(
            events,
            vec![
                PlaybackEvent::TrackEnded { track_id: 1 },
                PlaybackEvent::TrackStarted { track_id: 2 },
            ]
        );
        assert_eq!(h.service.snapshot().track_id, Some(2));
        assert_eq!(h.service.snapshot().status, TransportState::Playing);
    }

    #[test]
    fn last_track_end_emits_queue_finished_and_stops() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 1)
            .unwrap();

        h.engine.advance(Duration::from_millis(1000));
        let events = h.service.tick();

        assert_eq!(
            events,
            vec![
                PlaybackEvent::TrackEnded { track_id: 2 },
                PlaybackEvent::QueueFinished,
            ]
        );
        assert_eq!(h.service.snapshot().status, TransportState::Stopped);
    }

    #[test]
    fn truncated_track_is_detected_and_skipped() {
        // 完整复现实测场景：声明 3000 ms，实际只播到 1900 ms（64%）
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(3000)), entry(2, Some(3000))], 0)
            .unwrap();

        h.engine.advance(Duration::from_millis(1900));
        h.engine.finish_current();
        let events = h.service.tick();

        assert_eq!(events.len(), 2, "应产生失败事件 + 下一首开始");
        match &events[0] {
            PlaybackEvent::TrackFailed { track_id, code, .. } => {
                assert_eq!(*track_id, 1);
                assert_eq!(code, "PLAYBACK_TRUNCATED_FILE");
            }
            other => panic!("期望 TrackFailed，实际 {other:?}"),
        }
        assert_eq!(
            events[1],
            PlaybackEvent::TrackStarted { track_id: 2 },
            "截断后必须自动跳到下一首，而不是停下"
        );
        assert_eq!(h.service.snapshot().last_failure.unwrap().track_id, 1);
    }

    #[test]
    fn normal_end_is_not_mistaken_for_truncation() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(3000)), entry(2, Some(3000))], 0)
            .unwrap();

        h.engine.advance(Duration::from_millis(3000));
        let events = h.service.tick();

        assert_eq!(events[0], PlaybackEvent::TrackEnded { track_id: 1 });
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PlaybackEvent::TrackFailed { .. })),
            "正常播放完毕不得误判为截断：{events:?}"
        );
    }

    #[test]
    fn short_files_are_exempt_from_truncation_check() {
        // 1 秒 MP3 实测报 1152 ms；同理短文件比值波动大，不参与判定
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1152)), entry(2, Some(1152))], 0)
            .unwrap();

        h.engine.advance(Duration::from_millis(900));
        h.engine.finish_current();
        let events = h.service.tick();

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PlaybackEvent::TrackFailed { .. })),
            "短于 2 秒的曲目不应触发截断判定：{events:?}"
        );
    }

    #[test]
    fn decode_failure_on_auto_advance_skips_to_playable_track() {
        let h = harness();
        h.service
            .play_tracks(
                vec![entry(1, Some(1000)), entry(2, Some(1000)), entry(3, Some(1000))],
                0,
            )
            .unwrap();

        // 第二首加载必失败
        h.engine.fail_next_load("模拟损坏文件");
        h.engine.advance(Duration::from_millis(1000));
        let events = h.service.tick();

        assert_eq!(
            events,
            vec![
                PlaybackEvent::TrackEnded { track_id: 1 },
                PlaybackEvent::TrackFailed {
                    track_id: 2,
                    code: "PLAYBACK_DECODE_FAILED".into(),
                    message: "无法播放该文件，格式可能不受支持或文件已损坏。".into(),
                },
                PlaybackEvent::TrackStarted { track_id: 3 },
            ],
            "损坏曲目应被跳过，队列继续"
        );
    }

    #[test]
    fn all_tracks_broken_terminates_without_infinite_loop() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();

        // 后续每一次加载都失败：循环必须有界
        h.engine.advance(Duration::from_millis(1000));
        h.engine.fail_next_load("坏");
        h.engine.fail_next_load("坏");
        let events = h.service.tick();

        assert!(
            matches!(events.last(), Some(PlaybackEvent::QueueFinished)),
            "全损坏队列必须终止：{events:?}"
        );
        assert_eq!(h.service.snapshot().status, TransportState::Stopped);
    }

    #[test]
    fn toggle_switches_between_playing_and_paused() {
        let h = harness();
        h.service.play_tracks(vec![entry(1, Some(5000))], 0).unwrap();

        assert_eq!(h.service.snapshot().status, TransportState::Playing);
        h.service.toggle().unwrap();
        assert_eq!(h.service.snapshot().status, TransportState::Paused);
        h.service.toggle().unwrap();
        assert_eq!(h.service.snapshot().status, TransportState::Playing);
    }

    #[test]
    fn toggle_without_content_reports_nothing_playing() {
        let h = harness();
        assert!(matches!(
            h.service.toggle().unwrap_err(),
            AppError::Playback(PlaybackError::NothingPlaying)
        ));
    }

    #[test]
    fn manual_next_and_previous_move_within_bounds() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();

        assert_eq!(
            h.service.next().unwrap(),
            Some(PlaybackEvent::TrackStarted { track_id: 2 })
        );
        assert_eq!(h.service.snapshot().track_id, Some(2));

        assert_eq!(
            h.service.previous().unwrap(),
            Some(PlaybackEvent::TrackStarted { track_id: 1 })
        );
        assert_eq!(h.service.snapshot().track_id, Some(1));

        // 已在首曲：无操作而不报错
        assert_eq!(h.service.previous().unwrap(), None);
        assert_eq!(h.service.snapshot().track_id, Some(1));
    }

    #[test]
    fn next_at_last_track_stops_cleanly() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 1)
            .unwrap();

        assert_eq!(h.service.next().unwrap(), None);
        assert_eq!(h.service.snapshot().queue_length, 0, "末曲之后应清空队列");
        assert_eq!(h.service.snapshot().status, TransportState::Stopped);
    }

    // ══════════════════════════════════════════════════════════
    // 播放模式：循环与随机
    // ══════════════════════════════════════════════════════════

    #[test]
    fn repeat_mode_is_reflected_in_the_snapshot() {
        let h = harness();
        assert_eq!(h.service.snapshot().repeat, RepeatMode::Off, "默认应为顺序播放");

        for mode in [RepeatMode::All, RepeatMode::One, RepeatMode::Off] {
            h.service.set_repeat(mode);
            assert_eq!(h.service.snapshot().repeat, mode);
        }
    }

    #[test]
    fn shuffle_toggle_is_reflected_in_the_snapshot_and_keeps_current_track() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();

        assert!(!h.service.snapshot().shuffle, "默认应关闭随机");
        let before = h.service.snapshot().track_id;

        h.service.set_shuffle(true);
        let snap = h.service.snapshot();
        assert!(snap.shuffle);
        assert_eq!(snap.track_id, before, "开启随机不应打断当前曲目");

        h.service.set_shuffle(false);
        assert!(!h.service.snapshot().shuffle);
        assert_eq!(h.service.snapshot().track_id, before);
    }

    #[test]
    fn repeat_one_replays_the_same_track_on_natural_end() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();
        h.service.set_repeat(RepeatMode::One);

        h.engine.advance(Duration::from_millis(1000));
        let events = h.service.tick();

        assert_eq!(
            events,
            vec![
                PlaybackEvent::TrackEnded { track_id: 1 },
                PlaybackEvent::TrackStarted { track_id: 1 },
            ],
            "单曲循环应在自然结束后重播同一首"
        );
        assert_eq!(h.service.snapshot().track_id, Some(1));
        assert_eq!(h.service.snapshot().status, TransportState::Playing);
    }

    #[test]
    fn repeat_one_manual_next_still_advances() {
        // 单曲循环只作用于自动续播；手动"下一首"必须前进（通行约定）
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();
        h.service.set_repeat(RepeatMode::One);

        assert_eq!(
            h.service.next().unwrap(),
            Some(PlaybackEvent::TrackStarted { track_id: 2 }),
            "单曲循环下点下一首应前进，而不是重播"
        );
    }

    /// ⚠️ 关键回归：失败路径绝不能被单曲循环接管。
    ///
    /// 被判定为截断的文件每次重播都会再次提前结束 —— 若失败路径也重播，
    /// 就会无限循环播放同一段损坏音频。
    #[test]
    fn repeat_one_does_not_replay_a_truncated_track() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(3000)), entry(2, Some(3000))], 0)
            .unwrap();
        h.service.set_repeat(RepeatMode::One);

        h.engine.advance(Duration::from_millis(1900));
        h.engine.finish_current();
        let events = h.service.tick();

        assert_eq!(events.len(), 2, "应为 失败事件 + 下一首：{events:?}");
        assert!(
            matches!(&events[0], PlaybackEvent::TrackFailed { code, .. }
                if code == "PLAYBACK_TRUNCATED_FILE"),
            "首事件应为截断失败：{events:?}"
        );
        assert_eq!(
            events[1],
            PlaybackEvent::TrackStarted { track_id: 2 },
            "截断文件在单曲循环下也必须前进，不得重播"
        );
    }

    #[test]
    fn repeat_all_wraps_at_last_track_without_queue_finished() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 1)
            .unwrap();
        h.service.set_repeat(RepeatMode::All);

        h.engine.advance(Duration::from_millis(1000));
        let events = h.service.tick();

        assert_eq!(
            events,
            vec![
                PlaybackEvent::TrackEnded { track_id: 2 },
                PlaybackEvent::TrackStarted { track_id: 1 },
            ],
            "列表循环应在末曲回到首曲，且不得发出 QueueFinished"
        );
        assert_eq!(h.service.snapshot().track_id, Some(1));
    }

    #[test]
    fn repeat_all_manual_next_wraps_and_keeps_the_queue() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 1)
            .unwrap();
        h.service.set_repeat(RepeatMode::All);

        assert_eq!(
            h.service.next().unwrap(),
            Some(PlaybackEvent::TrackStarted { track_id: 1 })
        );

        let snap = h.service.snapshot();
        assert_eq!(snap.queue_length, 2, "列表循环下环绕不得清空队列");
        assert!(snap.has_next, "列表循环下任何位置都应有下一首");
        assert!(snap.has_previous, "列表循环下任何位置都应有上一首");
    }

    /// 无界环绕 + 全部加载失败 = 死循环。装载预算必须仍然有限。
    #[test]
    fn repeat_all_with_all_tracks_broken_still_terminates() {
        let h = harness();
        h.service
            .play_tracks(vec![entry(1, Some(1000)), entry(2, Some(1000))], 0)
            .unwrap();
        h.service.set_repeat(RepeatMode::All);

        h.engine.advance(Duration::from_millis(1000));
        h.engine.fail_every_load("坏");
        let events = h.service.tick();

        assert!(
            matches!(events.last(), Some(PlaybackEvent::QueueFinished)),
            "列表循环 + 全损坏队列也必须终止：{events:?}"
        );

        // 环绕不得让尝试次数失控：预算恒等于队列长度
        let attempts = events
            .iter()
            .filter(|e| matches!(e, PlaybackEvent::TrackFailed { .. }))
            .count();
        assert_eq!(attempts, 2, "尝试次数必须等于队列长度：{events:?}");

        assert_eq!(h.service.snapshot().status, TransportState::Stopped);
    }

    #[test]
    fn volume_is_clamped_and_reported() {
        let h = harness();
        h.service.set_volume(0.4).unwrap();
        assert!((h.service.snapshot().volume - 0.4).abs() < f32::EPSILON);

        h.service.set_volume(9.0).unwrap();
        assert!((h.service.snapshot().volume - 1.0).abs() < f32::EPSILON);

        assert!(matches!(
            h.service.set_volume(f32::NAN).unwrap_err(),
            AppError::InvalidArgument { .. }
        ));
    }

    #[test]
    fn seek_rejects_negative_and_out_of_range() {
        let h = harness();
        h.service.play_tracks(vec![entry(1, Some(1000))], 0).unwrap();

        assert!(matches!(
            h.service.seek(-1).unwrap_err(),
            AppError::InvalidArgument { .. }
        ));
        assert!(matches!(
            h.service.seek(5000).unwrap_err(),
            AppError::Playback(PlaybackError::SeekOutOfRange { .. })
        ));
        h.service.seek(500).unwrap();
        assert_eq!(h.service.snapshot().position_ms, 500);
    }

    #[test]
    fn position_is_capped_at_declared_duration() {
        let h = harness();
        h.service.play_tracks(vec![entry(1, Some(1000))], 0).unwrap();
        h.engine.advance(Duration::from_millis(1500)); // 超出声明时长
        assert_eq!(
            h.service.snapshot().position_ms,
            1000,
            "位置必须封顶在声明时长，避免界面显示超过 100%"
        );
    }

    #[test]
    fn stop_clears_queue_and_failure() {
        let h = harness();
        h.service.play_tracks(vec![entry(1, Some(3000))], 0).unwrap();
        h.engine.advance(Duration::from_millis(1000));
        h.engine.finish_current();
        h.service.tick();
        assert!(h.service.snapshot().last_failure.is_some());

        h.service.stop().unwrap();
        let snap = h.service.snapshot();
        assert_eq!(snap.queue_length, 0);
        assert_eq!(snap.track_id, None);
        assert!(snap.last_failure.is_none());
    }

    #[test]
    fn unavailable_engine_surfaces_no_output_device() {
        let engine = Arc::new(NullAudioEngine::unavailable());
        let service = PlaybackService::new(
            engine,
            Arc::new(FakeClock::new(0)),
            Arc::new(FakeRandom::new()),
        );

        let err = service.play_tracks(vec![entry(1, Some(1000))], 0).unwrap_err();
        assert_eq!(err.code(), "PLAYBACK_NO_OUTPUT_DEVICE");
        assert!(err.retryable(), "无设备是可重试的（用户可能插上耳机）");
    }

    #[test]
    fn snapshot_engine_description_is_present() {
        let h = harness();
        assert!(!h.service.snapshot().engine.is_empty());
    }
}
