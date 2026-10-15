//! 应用层：编排领域逻辑与基础设施，不直接触碰外部世界。

pub mod library_service;
pub mod playback_service;

use std::sync::Arc;

use crate::domain::ports::Clock;
use crate::infra::db::DbPool;
use crate::infra::time::SystemClock;

pub use playback_service::{PlaybackEvent, PlaybackService, PlaybackSnapshot};

/// 全局应用状态，由 Tauri 托管。
///
/// 持有的都是 `Arc` 包裹的共享对象，克隆开销极低，
/// 因此命令层可以放心地把它们移进阻塞任务或轮询线程。
#[derive(Clone)]
pub struct AppState {
    pub pool: DbPool,
    pub playback: Arc<PlaybackService>,
    pub clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new(pool: DbPool, playback: Arc<PlaybackService>) -> Self {
        Self {
            pool,
            playback,
            clock: Arc::new(SystemClock),
        }
    }
}
