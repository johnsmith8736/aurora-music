//! 端口定义（依赖倒置的接口）。
//!
//! 本模块只声明 trait 与值类型，**不含任何实现**，也不依赖
//! rodio / cpal / tauri 等外部库（docs/architecture.md §1.2 R1）。
//!
//! 实现位于 `infra/`，装配点在 `lib.rs`。

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use super::error::PlaybackError;
use super::model::TrackId;

/// 交给播放引擎的音频来源。
///
/// ⚠️ 当前仅支持本地文件。在线来源在 M5 引入 `ResolvedStream`（含 URL 校验），
/// 届时此处会扩展为枚举。**现在不提前抽象**——本地路径来自资料库，
/// 已经过规范化与存在性校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSource {
    pub track_id: TrackId,
    pub path: PathBuf,
    /// 容器头部声明的时长。
    ///
    /// 用于检测「被解码器接受、但实际提前结束」的损坏文件：
    /// 结束时若实际播放位置显著短于该值，判定为截断（见 `EngineEvent::TrackEnded`）。
    pub declared_duration: Option<Duration>,
}

/// 播放传输状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportState {
    /// 引擎空闲，从未加载过内容
    Idle,
    Playing,
    Paused,
    /// 已加载但已停止（区别于 Idle）
    Stopped,
    Error,
}

/// 引擎向服务层上报的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// 当前曲目播放结束。`position` 是结束瞬间的实际播放位置，
    /// **由服务层**拿它与 `AudioSource::declared_duration` 比对以判定截断。
    TrackEnded { track_id: TrackId, position: Duration },

    /// 加载或解码失败，无法播放该曲目。
    LoadFailed { track_id: TrackId, reason: String },

    /// 音频设备丢失（拔出、被占用等）。
    DeviceLost { reason: String },
}

/// 音频输出端口。
///
/// # 实现约束
///
/// - 所有方法**必须非阻塞**：实际音频工作由 rodio/cpal 的回调线程完成
/// - 实现必须 `Send + Sync`（已实测 rodio 的 `Player` 与 `MixerDeviceSink`
///   在 Linux/ALSA 上满足此要求，因此无需专用线程 + 通道）
/// - `take_events` 允许有副作用：实现会在其中检查播放状态并**一次性**
///   产出待处理事件（事件消费即清空，重复调用返回空）
pub trait AudioOutput: Send + Sync {
    /// 加载并立即开始播放。会替换当前内容。
    fn load_and_play(&self, source: AudioSource) -> Result<(), PlaybackError>;

    fn play(&self) -> Result<(), PlaybackError>;
    fn pause(&self) -> Result<(), PlaybackError>;
    fn stop(&self) -> Result<(), PlaybackError>;

    /// 跳转到指定位置。
    fn seek(&self, position: Duration) -> Result<(), PlaybackError>;

    /// 设置音量，取值 0.0–1.0。
    fn set_volume(&self, volume: f32) -> Result<(), PlaybackError>;
    fn volume(&self) -> f32;

    /// 当前播放位置。无内容时为 0。
    fn position(&self) -> Duration;

    fn state(&self) -> TransportState;

    /// 取出并清空自上次调用以来积累的事件。
    fn take_events(&self) -> Vec<EngineEvent>;

    /// 人类可读的引擎描述，用于诊断面板。
    fn describe(&self) -> String;
}

/// 时间源端口。
///
/// 存在的唯一目的：让依赖时间的逻辑可以在测试中确定性推进。
/// **`domain/` 与 `app/` 中禁止直接调用 `SystemTime::now()`。**
pub trait Clock: Send + Sync {
    /// 当前 Unix 时间戳（秒）。
    fn now_unix(&self) -> i64;
}

/// 随机源端口。
///
/// 与 `Clock` 同理：存在的唯一目的，是让依赖随机数的逻辑在测试中可以
/// 确定性地复现。**`domain/` 与 `app/` 中禁止直接读取系统随机源。**
///
/// 目前唯一的用途是给随机播放的置换提供种子（FR-3.5）。
pub trait RandomSource: Send + Sync {
    /// 生成一个洗牌种子。
    fn seed(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_state_serializes_lowercase() {
        let cases = [
            (TransportState::Idle, "\"idle\""),
            (TransportState::Playing, "\"playing\""),
            (TransportState::Paused, "\"paused\""),
            (TransportState::Stopped, "\"stopped\""),
            (TransportState::Error, "\"error\""),
        ];
        for (state, expected) in cases {
            assert_eq!(serde_json::to_string(&state).unwrap(), expected);
        }
    }
}
