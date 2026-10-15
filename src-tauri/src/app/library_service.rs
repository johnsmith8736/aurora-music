//! 资料库应用服务。
//!
//! 编排扫描、入库、查询。所有方法都是**阻塞**的：
//! 调用方（命令层）负责放进 `spawn_blocking`，避免阻塞异步运行时。

use std::path::Path;

use serde::Serialize;

use crate::domain::model::{LibraryRoot, RootId, Track, TrackId};
use crate::domain::queue::QueueEntry;
use crate::domain::{AppError, LibraryError};
use crate::infra::db::repo::{self, ScanStats};
use crate::infra::db::DbPool;
use crate::infra::scan::{self, FileError};

/// 扫描结果摘要，跨 IPC 返回给前端。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub root_id: RootId,
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub skipped: i64,
    pub failed: i64,
    pub missing: i64,
    pub duration_ms: i64,
    /// 失败文件列表（仅文件名，不含绝对路径）。
    pub errors: Vec<FileErrorDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileErrorDto {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub root_count: i64,
    pub track_count: i64,
}

impl From<FileError> for FileErrorDto {
    fn from(e: FileError) -> Self {
        FileErrorDto {
            name: e.name,
            reason: e.reason,
        }
    }
}

fn conn(pool: &DbPool) -> Result<crate::infra::db::DbConn, AppError> {
    pool.get().map_err(|e| {
        AppError::Storage(crate::domain::StorageError::Query {
            context: "pool.get",
            detail: e.to_string(),
        })
    })
}

/// 添加音乐目录。路径会被规范化。
pub fn add_root(pool: &DbPool, path: &str) -> Result<LibraryRoot, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidArgument {
            field: "path",
            detail: "路径为空".into(),
        });
    }

    let canonical = scan::canonicalize(Path::new(trimmed)).map_err(AppError::Library)?;
    let canonical_str = canonical.to_string_lossy().into_owned();

    let c = conn(pool)?;
    Ok(repo::insert_root(&c, &canonical_str)?)
}

pub fn list_roots(pool: &DbPool) -> Result<Vec<LibraryRoot>, AppError> {
    let c = conn(pool)?;
    Ok(repo::list_roots(&c)?)
}

pub fn remove_root(pool: &DbPool, root_id: RootId) -> Result<(), AppError> {
    let c = conn(pool)?;
    repo::delete_root(&c, root_id)?;
    Ok(())
}

/// 扫描一个根目录并把结果写入资料库。
///
/// 整个写入在**单个事务**内完成：扫描中断时不会留下半成品数据。
pub fn scan_root(pool: &DbPool, root_id: RootId) -> Result<ScanSummary, AppError> {
    let started = std::time::Instant::now();

    let root = {
        let c = conn(pool)?;
        repo::list_roots(&c)?
            .into_iter()
            .find(|r| r.id == root_id)
            .ok_or(AppError::Library(LibraryError::RootNotFound))?
    };

    // 1) 取出已知状态用于增量判定（只读连接，尽快释放）
    let known = {
        let c = conn(pool)?;
        repo::file_states(&c, root_id)?
    };

    // 2) 遍历磁盘（不持有数据库连接）
    let report = scan::scan_directory(Path::new(&root.path), &known)?;

    if report.stats.seen == 0 {
        return Err(AppError::Library(LibraryError::NoAudioFiles));
    }

    // 3) 单事务写入
    let mut c = conn(pool)?;
    let scan_id = repo::start_scan(&c, root_id)?;

    let tx = c.transaction().map_err(|e| {
        AppError::Storage(crate::domain::StorageError::Query {
            context: "scan/begin",
            detail: e.to_string(),
        })
    })?;

    for st in &report.tracks {
        repo::upsert_track(&tx, root_id, &st.meta)?;
    }
    for path in &report.missing {
        repo::mark_missing(&tx, path)?;
    }

    let stats = ScanStats {
        seen: report.stats.seen,
        added: report.stats.added,
        updated: report.stats.updated,
        skipped: report.stats.skipped,
        failed: report.stats.failed,
    };

    let summary_text = if report.errors.is_empty() {
        None
    } else {
        Some(format!("{} 个文件处理失败", report.errors.len()))
    };

    repo::finish_scan(&tx, scan_id, "completed", stats, summary_text.as_deref())?;

    tx.commit().map_err(|e| {
        AppError::Storage(crate::domain::StorageError::Query {
            context: "scan/commit",
            detail: e.to_string(),
        })
    })?;

    repo::touch_root_scan(&c, root_id)?;

    Ok(ScanSummary {
        root_id,
        seen: stats.seen,
        added: stats.added,
        updated: stats.updated,
        skipped: stats.skipped,
        failed: stats.failed,
        missing: report.missing.len() as i64,
        duration_ms: started.elapsed().as_millis() as i64,
        errors: report.errors.into_iter().map(Into::into).collect(),
    })
}

/// 列出曲目。`query` 非空时执行全文检索。
pub fn list_tracks(pool: &DbPool, query: Option<&str>, limit: i64) -> Result<Vec<Track>, AppError> {
    let c = conn(pool)?;
    let limit = limit.clamp(1, 5000);
    Ok(repo::list_tracks(&c, query, limit)?)
}

pub fn stats(pool: &DbPool) -> Result<LibraryStats, AppError> {
    let c = conn(pool)?;
    Ok(LibraryStats {
        root_count: repo::list_roots(&c)?.len() as i64,
        track_count: repo::count_tracks(&c)?,
    })
}

/// 按给定顺序把曲目 ID 解析为播放队列条目。
///
/// 任何一个 ID 不存在都返回错误 —— 静默跳过会让"点了 10 首只播了 8 首"
/// 这种问题难以定位。
pub fn load_queue_entries(
    pool: &DbPool,
    track_ids: &[TrackId],
) -> Result<Vec<QueueEntry>, AppError> {
    let c = conn(pool)?;
    let mut out = Vec::with_capacity(track_ids.len());

    for id in track_ids {
        let track = repo::get_track(&c, *id)?
            .ok_or(AppError::Library(LibraryError::TrackNotFound { id: *id }))?;

        out.push(QueueEntry {
            track_id: track.id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            path: std::path::PathBuf::from(track.path),
            duration_ms: track.duration_ms,
        });
    }

    Ok(out)
}
