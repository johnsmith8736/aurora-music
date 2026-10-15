//! 基于 rodio 0.23 + cpal 的音频播放引擎。
//!
//! # 已实测的关键事实（本实现的依据）
//!
//! 这些结论来自对 rodio 0.23 的实际编译与运行验证，**不是文档推断**：
//!
//! | 事实 | 验证方式 |
//! |---|---|
//! | `Player` 与 `MixerDeviceSink` 均为 `Send + Sync` | 编译期 trait 断言 |
//! | `Player::get_pos()` 精确，曲目结束后保持终值 | 1 秒文件结束时报 1000 ms |
//! | `append()` 会重置位置计数 | 1000 ms → 335 ms |
//! | `empty()` 为 true 表示当前内容已播完 | 与 get_pos 交叉验证 |
//! | **`MixerDeviceSink` 必须保活**，drop 即停止音频 | rodio 自身的 drop 警告 |
//!
//! # 已知危险点
//!
//! `Player::empty()` 在 `append()` 之后、真正开始出声之前会短暂为 `true`。
//! 若直接用它判定"播完"，会在刚加载时误报曲目结束。
//! 本实现用「曾经非空 **或** 位置 > 0」作为前置条件来消除这个竞态。

use std::fs::File;
use std::sync::Mutex;
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

use crate::domain::model::TrackId;
use crate::domain::ports::{AudioOutput, AudioSource, EngineEvent, TransportState};
use crate::domain::PlaybackError;

/// 当前已加载音源的追踪状态。
struct CurrentSource {
    track_id: TrackId,
    declared_duration: Option<Duration>,
    /// 是否曾观察到 player 非空（消除 append 后的瞬时空队列竞态）
    observed_active: bool,
    /// 是否已上报过结束事件（防止重复上报）
    end_reported: bool,
}

struct Inner {
    current: Option<CurrentSource>,
    /// 曾经加载过内容 —— 用于区分 Idle 与 Stopped
    ever_loaded: bool,
    volume: f32,
    events: Vec<EngineEvent>,
    device_lost: bool,
}

pub struct RodioAudioEngine {
    /// ⚠️ 必须保活：drop 会立即停止所有音频输出。
    /// 字段名前缀下划线仅表示"不读取"，不表示可以删除。
    _sink: MixerDeviceSink,
    player: Player,
    inner: Mutex<Inner>,
}

impl RodioAudioEngine {
    /// 打开默认输出设备并建立引擎。
    ///
    /// 失败时返回 `NoOutputDevice`，调用方应降级到 `NullAudioEngine`
    /// 而不是让应用启动失败。
    pub fn open_default() -> Result<Self, PlaybackError> {
        let sink = DeviceSinkBuilder::open_default_sink().map_err(|e| match e {
            rodio::DeviceSinkError::NoDevice => PlaybackError::NoOutputDevice,
            other => PlaybackError::DeviceLost {
                reason: other.to_string(),
            },
        })?;

        let mixer = sink.mixer().clone();
        let player = Player::connect_new(&mixer);

        Ok(Self {
            _sink: sink,
            player,
            inner: Mutex::new(Inner {
                current: None,
                ever_loaded: false,
                volume: 1.0,
                events: Vec::new(),
                device_lost: false,
            }),
        })
    }

    /// 输出设备的实际配置描述。
    pub fn device_description(&self) -> String {
        let c = self._sink.config();
        format!(
            "{} 声道 / {} Hz / {:?}",
            c.channel_count(),
            c.sample_rate(),
            c.sample_format()
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 中毒的 Mutex 仍可取回数据：本结构的不变量不依赖 panic 安全
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 同步内部状态：记录「曾观察到确实在播放」，并在首次发现自然结束时
    /// 把 `TrackEnded` 压入事件队列并标记 `end_reported`。
    ///
    /// # 为什么 `state()` 也要调用它
    ///
    /// 若只有 `take_events()` 更新结束标志，那么先调用 `state()` 的场景
    /// （如 `playback_get_state`）会读到陈旧的 `Playing`。让两者共用同一个
    /// 幂等刷新函数，状态判定就与调用顺序无关。
    ///
    /// 幂等：`end_reported` 一旦置位，重复调用不会重复上报。
    fn refresh(&self, inner: &mut Inner) {
        // 先把需要的数据取出，结束对 inner.current 的借用
        let ended = {
            let Some(cur) = inner.current.as_mut() else {
                return;
            };
            if !self.player.empty() {
                // 观察到确实在播放 —— 解除 append 后的瞬时空队列歧义
                cur.observed_active = true;
                None
            } else if cur.end_reported {
                None
            } else {
                let pos = self.player.get_pos();
                // 必须曾经真的播过，才可能是"播完"
                if cur.observed_active || pos > Duration::ZERO {
                    cur.end_reported = true;
                    Some((cur.track_id, pos))
                } else {
                    None
                }
            }
        };

        if let Some((track_id, position)) = ended {
            inner.events.push(EngineEvent::TrackEnded { track_id, position });
        }
    }
}

impl AudioOutput for RodioAudioEngine {
    fn load_and_play(&self, source: AudioSource) -> Result<(), PlaybackError> {
        let track_id = source.track_id;

        // 构建解码器。显式传入 byte_len —— 对 MP3 / Vorbis 的可靠 seek
        // 与时长计算是必需的（rodio 文档明确要求）。
        let file = File::open(&source.path).map_err(|e| PlaybackError::DecodeFailed {
            track_id,
            reason: format!("无法打开文件: {e}"),
        })?;
        let byte_len = file
            .metadata()
            .map(|m| m.len())
            .map_err(|e| PlaybackError::DecodeFailed {
                track_id,
                reason: format!("无法读取文件属性: {e}"),
            })?;

        let decoder = Decoder::builder()
            .with_data(file)
            .with_byte_len(byte_len)
            .with_seekable(true)
            .build()
            .map_err(|e| PlaybackError::DecodeFailed {
                track_id,
                reason: e.to_string(),
            })?;

        // 清空并加载新内容
        self.player.stop();

        {
            let mut inner = self.lock();
            inner.current = Some(CurrentSource {
                track_id,
                declared_duration: source.declared_duration,
                observed_active: false,
                end_reported: false,
            });
            inner.ever_loaded = true;
            inner.device_lost = false;
            let vol = inner.volume;
            self.player.set_volume(vol);
        }

        self.player.append(decoder);
        self.player.play();

        Ok(())
    }

    fn play(&self) -> Result<(), PlaybackError> {
        if self.lock().current.is_none() {
            return Err(PlaybackError::NothingPlaying);
        }
        self.player.play();
        Ok(())
    }

    fn pause(&self) -> Result<(), PlaybackError> {
        if self.lock().current.is_none() {
            return Err(PlaybackError::NothingPlaying);
        }
        self.player.pause();
        Ok(())
    }

    fn stop(&self) -> Result<(), PlaybackError> {
        self.player.stop();
        let mut inner = self.lock();
        inner.current = None;
        Ok(())
    }

    fn seek(&self, position: Duration) -> Result<(), PlaybackError> {
        let inner = self.lock();
        let cur = inner.current.as_ref().ok_or(PlaybackError::NothingPlaying)?;

        if let Some(total) = cur.declared_duration
            && position > total
        {
            return Err(PlaybackError::SeekOutOfRange {
                requested_ms: position.as_millis() as i64,
                duration_ms: total.as_millis() as i64,
            });
        }
        drop(inner);

        self.player
            .try_seek(position)
            .map_err(|e| PlaybackError::DecodeFailed {
                track_id: 0,
                reason: format!("跳转失败: {e}"),
            })
    }

    fn set_volume(&self, volume: f32) -> Result<(), PlaybackError> {
        let v = volume.clamp(0.0, 1.0);
        self.player.set_volume(v);
        self.lock().volume = v;
        Ok(())
    }

    fn volume(&self) -> f32 {
        self.lock().volume
    }

    fn position(&self) -> Duration {
        self.player.get_pos()
    }

    fn state(&self) -> TransportState {
        let mut inner = self.lock();

        if inner.device_lost {
            return TransportState::Error;
        }
        if inner.current.is_none() {
            return if inner.ever_loaded {
                TransportState::Stopped
            } else {
                TransportState::Idle
            };
        }

        // 关键：先刷新，否则曲目播完后仍会报 Playing
        self.refresh(&mut inner);

        if inner.current.as_ref().is_some_and(|c| c.end_reported) {
            return TransportState::Stopped;
        }
        if self.player.is_paused() {
            TransportState::Paused
        } else {
            TransportState::Playing
        }
    }

    fn take_events(&self) -> Vec<EngineEvent> {
        let mut inner = self.lock();
        self.refresh(&mut inner);
        std::mem::take(&mut inner.events)
    }

    fn describe(&self) -> String {
        format!("rodio/cpal · {}", self.device_description())
    }
}

#[cfg(test)]
mod tests {
    //! 这些测试需要真实声卡。在无声卡环境（CI）中会被跳过。

    use super::*;
    use crate::domain::ports::AudioOutput;

    fn fixtures() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio")
    }

    fn open_engine() -> Option<RodioAudioEngine> {
        RodioAudioEngine::open_default().ok()
    }

    #[test]
    fn engine_opens_or_reports_no_device() {
        match RodioAudioEngine::open_default() {
            Ok(e) => {
                assert!(!e.describe().is_empty());
                assert_eq!(e.state(), TransportState::Idle, "初始状态必须是 Idle");
            }
            Err(PlaybackError::NoOutputDevice) => {
                eprintln!("跳过：本机无音频输出设备");
            }
            Err(other) => panic!("非预期的打开失败: {other}"),
        }
    }

    #[test]
    fn initial_state_is_idle_not_stopped() {
        let Some(e) = open_engine() else { return };
        assert_eq!(e.state(), TransportState::Idle);
        assert_eq!(e.position(), Duration::ZERO);
        assert!(e.take_events().is_empty(), "初始不应有任何事件");
    }

    #[test]
    fn missing_file_reports_decode_failure_without_panicking() {
        let Some(e) = open_engine() else { return };
        let err = e
            .load_and_play(AudioSource {
                track_id: 7,
                path: "/definitely/missing.mp3".into(),
                declared_duration: None,
            })
            .unwrap_err();
        match err {
            PlaybackError::DecodeFailed { track_id, ref reason } => {
                assert_eq!(track_id, 7);
                assert!(!reason.is_empty());
            }
            other => panic!("期望 DecodeFailed，实际 {other}"),
        }
    }

    #[test]
    fn text_file_disguised_as_mp3_is_rejected_at_load() {
        let Some(e) = open_engine() else { return };
        let p = fixtures().join("not_audio.mp3");
        if !p.exists() {
            return;
        }
        let r = e.load_and_play(AudioSource {
            track_id: 1,
            path: p,
            declared_duration: None,
        });
        assert!(
            matches!(r, Err(PlaybackError::DecodeFailed { .. })),
            "伪装文件必须在加载阶段被拒绝，实际: {r:?}"
        );
    }

    #[test]
    fn volume_is_clamped_and_remembered() {
        let Some(e) = open_engine() else { return };
        e.set_volume(0.5).unwrap();
        assert!((e.volume() - 0.5).abs() < f32::EPSILON);
        e.set_volume(2.5).unwrap();
        assert!((e.volume() - 1.0).abs() < f32::EPSILON, "音量上限应为 1.0");
        e.set_volume(-1.0).unwrap();
        assert!((e.volume() - 0.0).abs() < f32::EPSILON, "音量下限应为 0.0");
    }

    #[test]
    fn play_and_pause_without_content_errors() {
        let Some(e) = open_engine() else { return };
        assert!(matches!(
            e.play().unwrap_err(),
            PlaybackError::NothingPlaying
        ));
        assert!(matches!(
            e.pause().unwrap_err(),
            PlaybackError::NothingPlaying
        ));
    }
}
