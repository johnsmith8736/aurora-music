//! 音频输出实现。

pub mod null_engine;
pub mod rodio_engine;

use std::sync::Arc;

use crate::domain::ports::AudioOutput;

pub use null_engine::NullAudioEngine;
pub use rodio_engine::RodioAudioEngine;

/// 打开可用的最佳音频引擎。
///
/// 打不开真实设备时**降级**到 `NullAudioEngine::unavailable()` 而不是
/// 让应用启动失败 —— 用户仍可浏览资料库，播放时会看到明确的无设备错误
/// （FR-2.7）。
pub fn open_best_engine() -> Arc<dyn AudioOutput> {
    match RodioAudioEngine::open_default() {
        Ok(engine) => {
            eprintln!("[aurora-music][audio] 引擎就绪: {}", engine.describe());
            Arc::new(engine)
        }
        Err(err) => {
            eprintln!(
                "[aurora-music][audio] 无法打开音频设备，进入静默模式: {err}"
            );
            Arc::new(NullAudioEngine::unavailable())
        }
    }
}
