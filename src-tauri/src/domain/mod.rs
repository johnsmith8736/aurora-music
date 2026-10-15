//! 领域层：纯逻辑，零 I/O。
//!
//! 依赖规则（docs/architecture.md §1.2 R1）：
//! 本层**不得**依赖 tauri / rusqlite / lofty / walkdir 等任何外部库。

pub mod error;
pub mod model;
pub mod ports;
pub mod queue;

pub use error::{AppError, ErrorPayload, LibraryError, PlaybackError, StorageError};
pub use ports::{AudioOutput, AudioSource, Clock, EngineEvent, TransportState};
pub use queue::{PlayQueue, QueueEntry};
pub use model::{
    compute_album_key, is_audio_extension, title_from_filename, LibraryRoot, RootId, Track,
    TrackId, TrackMetadata, AUDIO_EXTENSIONS,
};
