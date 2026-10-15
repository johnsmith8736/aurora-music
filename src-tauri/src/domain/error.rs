//! 领域错误类型。
//!
//! 设计要点（见 docs/architecture.md §6）：
//! - 每个错误有稳定的机器可读 code，前端据此分支，不依赖文案
//! - 面向用户的消息已脱敏，不含绝对路径
//! - 本模块禁止 unwrap/expect/panic

use serde::Serialize;

use super::model::TrackId;

/// 资料库相关错误。
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("音乐目录不存在或不可访问")]
    RootNotFound,

    #[error("目录中没有任何可识别的音频文件")]
    NoAudioFiles,

    #[error("无法读取目录内容")]
    WalkFailed { detail: String },

    #[error("曲目不存在")]
    TrackNotFound { id: TrackId },

    #[error("扫描过程中止")]
    ScanCancelled,
}

/// 播放相关错误。
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("没有可用的音频输出设备")]
    NoOutputDevice,

    #[error("无法解码该文件")]
    DecodeFailed {
        track_id: TrackId,
        reason: String,
    },

    #[error("文件在播放中提前结束，可能已损坏")]
    TruncatedFile {
        track_id: TrackId,
        declared_ms: i64,
        actual_ms: i64,
    },

    #[error("跳转位置超出范围")]
    SeekOutOfRange { requested_ms: i64, duration_ms: i64 },

    #[error("音频设备连接中断")]
    DeviceLost { reason: String },

    #[error("当前没有播放内容")]
    NothingPlaying,

    #[error("队列为空")]
    EmptyQueue,
}

/// 存储层错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("数据库迁移失败")]
    Migration { from: u32, to: u32, detail: String },

    #[error("数据库版本高于本程序，请升级 Aurora Music")]
    SchemaTooNew { db_version: u32, app_version: u32 },

    #[error("数据库操作失败")]
    Query {
        context: &'static str,
        detail: String,
    },

    #[error("无法初始化数据库连接池")]
    PoolInit { detail: String },
}

/// 应用顶层错误。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Library(#[from] LibraryError),

    #[error(transparent)]
    Playback(#[from] PlaybackError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("参数无效")]
    InvalidArgument { field: &'static str, detail: String },

    #[error("内部错误")]
    Internal { detail: String },
}

impl AppError {
    /// 稳定的机器可读错误码。**不得随版本重命名**（前端依赖它做分支）。
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Library(e) => match e {
                LibraryError::RootNotFound => "LIBRARY_ROOT_NOT_FOUND",
                LibraryError::NoAudioFiles => "LIBRARY_NO_AUDIO_FILES",
                LibraryError::WalkFailed { .. } => "LIBRARY_WALK_FAILED",
                LibraryError::TrackNotFound { .. } => "LIBRARY_TRACK_NOT_FOUND",
                LibraryError::ScanCancelled => "LIBRARY_SCAN_CANCELLED",
            },
            AppError::Playback(e) => match e {
                PlaybackError::NoOutputDevice => "PLAYBACK_NO_OUTPUT_DEVICE",
                PlaybackError::DecodeFailed { .. } => "PLAYBACK_DECODE_FAILED",
                PlaybackError::TruncatedFile { .. } => "PLAYBACK_TRUNCATED_FILE",
                PlaybackError::SeekOutOfRange { .. } => "PLAYBACK_SEEK_OUT_OF_RANGE",
                PlaybackError::DeviceLost { .. } => "PLAYBACK_DEVICE_LOST",
                PlaybackError::NothingPlaying => "PLAYBACK_NOTHING_PLAYING",
                PlaybackError::EmptyQueue => "PLAYBACK_EMPTY_QUEUE",
            },
            AppError::Storage(e) => match e {
                StorageError::Migration { .. } => "STORAGE_MIGRATION_FAILED",
                StorageError::SchemaTooNew { .. } => "STORAGE_SCHEMA_TOO_NEW",
                StorageError::Query { .. } => "STORAGE_QUERY_FAILED",
                StorageError::PoolInit { .. } => "STORAGE_POOL_INIT_FAILED",
            },
            AppError::InvalidArgument { .. } => "INVALID_ARGUMENT",
            AppError::Internal { .. } => "INTERNAL",
        }
    }

    /// 前端是否应提示用户"可以重试"。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            AppError::Library(LibraryError::ScanCancelled)
                | AppError::Playback(PlaybackError::NoOutputDevice)
                | AppError::Playback(PlaybackError::DeviceLost { .. })
                | AppError::Storage(StorageError::Query { .. })
                | AppError::Internal { .. }
        )
    }
}

/// 面向用户的脱敏消息。
///
/// 绝对路径、完整 URL、凭据一律不得出现在这里（NFR-4.5）。
impl AppError {
    pub fn user_message(&self) -> String {
        match self {
            AppError::Library(LibraryError::RootNotFound) => {
                "该目录不存在或没有读取权限，请重新选择。".into()
            }
            AppError::Library(LibraryError::NoAudioFiles) => {
                "该目录下没有找到 MP3 / FLAC / WAV / OGG / M4A 音频文件。".into()
            }
            AppError::Library(LibraryError::WalkFailed { .. }) => {
                "读取目录时出错，请检查目录权限。".into()
            }
            AppError::Library(LibraryError::TrackNotFound { .. }) => {
                "该曲目已不在资料库中。".into()
            }
            AppError::Library(LibraryError::ScanCancelled) => "扫描已取消。".into(),
            AppError::Playback(PlaybackError::NoOutputDevice) => {
                "没有找到可用的音频输出设备，请检查系统声音设置。".into()
            }
            AppError::Playback(PlaybackError::DecodeFailed { .. }) => {
                "无法播放该文件，格式可能不受支持或文件已损坏。".into()
            }
            AppError::Playback(PlaybackError::TruncatedFile { .. }) => {
                "该文件在播放中途意外结束，可能已损坏，已跳到下一首。".into()
            }
            AppError::Playback(PlaybackError::SeekOutOfRange { .. }) => {
                "无法跳转到该位置。".into()
            }
            AppError::Playback(PlaybackError::DeviceLost { .. }) => {
                "音频设备已断开连接。".into()
            }
            AppError::Playback(PlaybackError::NothingPlaying) => "当前没有正在播放的内容。".into(),
            AppError::Playback(PlaybackError::EmptyQueue) => "播放队列为空。".into(),
            AppError::Storage(StorageError::Migration { .. }) => {
                "数据库升级失败，资料库可能无法使用。".into()
            }
            AppError::Storage(StorageError::SchemaTooNew { .. }) => {
                "资料库由更新版本的 Aurora Music 创建，请升级后再打开。".into()
            }
            AppError::Storage(StorageError::Query { .. }) => {
                "读写资料库时出错，请重试。".into()
            }
            AppError::Storage(StorageError::PoolInit { .. }) => {
                "无法打开资料库文件，请检查磁盘空间与权限。".into()
            }
            AppError::InvalidArgument { field, .. } => format!("参数 {field} 无效。"),
            AppError::Internal { .. } => "发生了内部错误，详情已记录到日志。".into(),
        }
    }
}

/// 跨 IPC 传输的错误载荷。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl From<AppError> for ErrorPayload {
    fn from(e: AppError) -> Self {
        // 内部细节只进日志，不进载荷
        tracing_error_detail(&e);
        ErrorPayload {
            code: e.code().to_string(),
            message: e.user_message(),
            retryable: e.retryable(),
        }
    }
}

/// 把完整的错误细节写入日志（含内部信息），供诊断用。
fn tracing_error_detail(e: &AppError) {
    // TODO(logging): 接入 tracing 后改为结构化输出。
    // 当前阶段先走 stderr，避免把内部细节暴露给前端。
    eprintln!("[aurora-music][error] code={} detail={e:?}", e.code());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_and_unique() {
        let samples = [
            AppError::Library(LibraryError::RootNotFound),
            AppError::Library(LibraryError::NoAudioFiles),
            AppError::Library(LibraryError::WalkFailed {
                detail: "x".into(),
            }),
            AppError::Library(LibraryError::TrackNotFound { id: 1 }),
            AppError::Library(LibraryError::ScanCancelled),
            AppError::Storage(StorageError::PoolInit {
                detail: "x".into(),
            }),
        ];
        let codes: Vec<_> = samples.iter().map(|e| e.code()).collect();
        for c in &codes {
            assert!(!c.is_empty());
            assert!(
                c.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_'),
                "错误码必须是 SCREAMING_SNAKE_CASE: {c}"
            );
        }
        // 这些是不同的错误，不应共用 code
        assert_ne!(codes[0], codes[1]);
    }

    #[test]
    fn user_message_does_not_leak_paths() {
        let e = AppError::Library(LibraryError::WalkFailed {
            detail: "/home/arch/secret/path: Permission denied".into(),
        });
        let msg = e.user_message();
        assert!(
            !msg.contains("/home/arch"),
            "用户可见消息不得包含绝对路径，实际为: {msg}"
        );
    }

    #[test]
    fn retryable_flag_matches_design() {
        assert!(AppError::Library(LibraryError::ScanCancelled).retryable());
        assert!(!AppError::Library(LibraryError::RootNotFound).retryable());
        assert!(!AppError::Storage(StorageError::SchemaTooNew {
            db_version: 2,
            app_version: 1
        })
        .retryable());
    }
}
