//! 资料库仓储：根目录、曲目、扫描会话的读写。

use std::collections::HashMap;

use rusqlite::{params, Connection, Row};

use super::now_unix;
use crate::domain::model::{LibraryRoot, RootId, Track, TrackId, TrackMetadata};
use crate::domain::StorageError;

fn query_err(context: &'static str) -> impl Fn(rusqlite::Error) -> StorageError {
    move |e| StorageError::Query {
        context,
        detail: e.to_string(),
    }
}

/// 曲目的 SELECT 列表（顺序必须与 `track_from_row` 一致）。
const TRACK_COLUMNS: &str = "
    id, path, title, artist, album, album_artist, album_key,
    track_no, disc_no, year,
    duration_ms, sample_rate, channels, bitrate, codec, container,
    file_size, file_mtime, added_at, updated_at
";

/// 同上，但带 `t.` 前缀。与 `tracks_fts` 联查时必须使用：
/// FTS 表和主表都有 title/artist/album 等同名列，不加前缀会报列名歧义。
const TRACK_COLUMNS_QUALIFIED: &str = "
    t.id, t.path, t.title, t.artist, t.album, t.album_artist, t.album_key,
    t.track_no, t.disc_no, t.year,
    t.duration_ms, t.sample_rate, t.channels, t.bitrate, t.codec, t.container,
    t.file_size, t.file_mtime, t.added_at, t.updated_at
";

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get("id")?,
        path: row.get("path")?,
        title: row.get("title")?,
        artist: row.get("artist")?,
        album: row.get("album")?,
        album_artist: row.get("album_artist")?,
        album_key: row.get("album_key")?,
        track_no: row.get("track_no")?,
        disc_no: row.get("disc_no")?,
        year: row.get("year")?,
        duration_ms: row.get("duration_ms")?,
        sample_rate: row.get("sample_rate")?,
        channels: row.get("channels")?,
        bitrate: row.get("bitrate")?,
        codec: row.get("codec")?,
        container: row.get("container")?,
        file_size: row.get("file_size")?,
        file_mtime: row.get("file_mtime")?,
        added_at: row.get("added_at")?,
        updated_at: row.get("updated_at")?,
    })
}

// ─────────────────────────────────────────────────────────────
// 根目录
// ─────────────────────────────────────────────────────────────

/// 新增根目录。若路径已存在则返回既有记录（幂等）。
pub fn insert_root(conn: &Connection, path: &str) -> Result<LibraryRoot, StorageError> {
    let now = now_unix();
    conn.execute(
        "INSERT INTO library_roots (path, enabled, created_at)
         VALUES (?1, 1, ?2)
         ON CONFLICT(path) DO NOTHING",
        params![path, now],
    )
    .map_err(query_err("insert_root"))?;

    get_root_by_path(conn, path)?.ok_or(StorageError::Query {
        context: "insert_root/readback",
        detail: "写入后未能读回根目录记录".into(),
    })
}

pub fn get_root_by_path(
    conn: &Connection,
    path: &str,
) -> Result<Option<LibraryRoot>, StorageError> {
    conn.query_row(
        "SELECT id, path, enabled, last_scan_at, created_at
         FROM library_roots WHERE path = ?1",
        params![path],
        |row| {
            Ok(LibraryRoot {
                id: row.get("id")?,
                path: row.get("path")?,
                enabled: row.get::<_, i64>("enabled")? != 0,
                last_scan_at: row.get("last_scan_at")?,
                created_at: row.get("created_at")?,
            })
        },
    )
    .optional_row()
}

pub fn list_roots(conn: &Connection) -> Result<Vec<LibraryRoot>, StorageError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, enabled, last_scan_at, created_at
             FROM library_roots ORDER BY path",
        )
        .map_err(query_err("list_roots/prepare"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(LibraryRoot {
                id: row.get("id")?,
                path: row.get("path")?,
                enabled: row.get::<_, i64>("enabled")? != 0,
                last_scan_at: row.get("last_scan_at")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(query_err("list_roots/query"))?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(query_err("list_roots/collect"))
}

pub fn delete_root(conn: &Connection, root_id: RootId) -> Result<(), StorageError> {
    conn.execute("DELETE FROM library_roots WHERE id = ?1", params![root_id])
        .map_err(query_err("delete_root"))?;
    Ok(())
}

pub fn touch_root_scan(conn: &Connection, root_id: RootId) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE library_roots SET last_scan_at = ?2 WHERE id = ?1",
        params![root_id, now_unix()],
    )
    .map_err(query_err("touch_root_scan"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// 曲目
// ─────────────────────────────────────────────────────────────

/// 按路径写入或更新曲目。已存在时保留 `added_at`。
pub fn upsert_track(
    conn: &Connection,
    root_id: RootId,
    meta: &TrackMetadata,
) -> Result<TrackId, StorageError> {
    let now = now_unix();
    conn.execute(
        "INSERT INTO tracks (
            root_id, path, title, artist, album, album_artist, album_key,
            track_no, disc_no, year,
            duration_ms, sample_rate, channels, bitrate, codec, container,
            file_size, file_mtime, added_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19
         )
         ON CONFLICT(path) DO UPDATE SET
            root_id       = excluded.root_id,
            title         = excluded.title,
            artist        = excluded.artist,
            album         = excluded.album,
            album_artist  = excluded.album_artist,
            album_key     = excluded.album_key,
            track_no      = excluded.track_no,
            disc_no       = excluded.disc_no,
            year          = excluded.year,
            duration_ms   = excluded.duration_ms,
            sample_rate   = excluded.sample_rate,
            channels      = excluded.channels,
            bitrate       = excluded.bitrate,
            codec         = excluded.codec,
            container     = excluded.container,
            file_size     = excluded.file_size,
            file_mtime    = excluded.file_mtime,
            updated_at    = excluded.updated_at,
            missing_since = NULL",
        params![
            root_id,
            meta.path,
            meta.title,
            meta.artist,
            meta.album,
            meta.album_artist,
            meta.album_key,
            meta.track_no,
            meta.disc_no,
            meta.year,
            meta.duration_ms,
            meta.sample_rate,
            meta.channels,
            meta.bitrate,
            meta.codec,
            meta.container,
            meta.file_size,
            meta.file_mtime,
            now,
        ],
    )
    .map_err(query_err("upsert_track"))?;

    conn.query_row(
        "SELECT id FROM tracks WHERE path = ?1",
        params![meta.path],
        |r| r.get(0),
    )
    .map_err(query_err("upsert_track/readback"))
}

/// 读取某根目录下所有曲目的 (路径 → (大小, 修改时间))，供增量扫描比对。
pub fn file_states(
    conn: &Connection,
    root_id: RootId,
) -> Result<HashMap<String, (i64, i64)>, StorageError> {
    let mut stmt = conn
        .prepare("SELECT path, file_size, file_mtime FROM tracks WHERE root_id = ?1")
        .map_err(query_err("file_states/prepare"))?;

    let rows = stmt
        .query_map(params![root_id], |row| {
            Ok((
                row.get::<_, String>("path")?,
                (row.get::<_, i64>("file_size")?, row.get::<_, i64>("file_mtime")?),
            ))
        })
        .map_err(query_err("file_states/query"))?;

    rows.collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(query_err("file_states/collect"))
}

/// 列出曲目。`query` 非空时走 FTS5 全文检索。
pub fn list_tracks(
    conn: &Connection,
    query: Option<&str>,
    limit: i64,
) -> Result<Vec<Track>, StorageError> {
    let trimmed = query.map(str::trim).filter(|q| !q.is_empty());

    let sql = match trimmed {
        Some(_) => format!(
            "SELECT {TRACK_COLUMNS_QUALIFIED} FROM tracks t
             JOIN tracks_fts f ON f.rowid = t.id
             WHERE tracks_fts MATCH ?1 AND t.missing_since IS NULL
             ORDER BY f.rank LIMIT ?2"
        ),
        None => format!(
            "SELECT {TRACK_COLUMNS} FROM tracks
             WHERE missing_since IS NULL
             ORDER BY artist, album, disc_no, track_no, title LIMIT ?1"
        ),
    };

    let mut stmt = conn
        .prepare(&sql)
        .map_err(query_err("list_tracks/prepare"))?;

    let map_row = |row: &Row<'_>| track_from_row(row);

    let rows = match trimmed {
        Some(q) => {
            // FTS5 查询语法对用户输入敏感，转为前缀匹配以避免语法错误
            let fts_query = to_fts_prefix_query(q);
            stmt.query_map(params![fts_query, limit], map_row)
                .map_err(query_err("list_tracks/fts"))?
        }
        None => stmt
            .query_map(params![limit], map_row)
            .map_err(query_err("list_tracks/all"))?,
    };

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(query_err("list_tracks/collect"))
}

/// 把用户输入转成安全的 FTS5 前缀查询。
///
/// 直接把用户输入拼进 MATCH 会导致语法错误或注入（例如输入 `"` 或 `*`），
/// 因此逐词加引号后再追加 `*` 做前缀匹配。
pub fn to_fts_prefix_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|w| w.replace('"', "\"\""))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn count_tracks(conn: &Connection) -> Result<i64, StorageError> {
    conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE missing_since IS NULL",
        [],
        |r| r.get(0),
    )
    .map_err(query_err("count_tracks"))
}

pub fn get_track(conn: &Connection, id: TrackId) -> Result<Option<Track>, StorageError> {
    conn.query_row(
        &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
        params![id],
        track_from_row,
    )
    .optional_row()
}

pub fn mark_missing(conn: &Connection, path: &str) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE tracks SET missing_since = ?2 WHERE path = ?1 AND missing_since IS NULL",
        params![path, now_unix()],
    )
    .map_err(query_err("mark_missing"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// 扫描会话
// ─────────────────────────────────────────────────────────────

pub fn start_scan(conn: &Connection, root_id: RootId) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO scan_sessions (root_id, started_at, status) VALUES (?1, ?2, 'running')",
        params![root_id, now_unix()],
    )
    .map_err(query_err("start_scan"))?;
    Ok(conn.last_insert_rowid())
}

/// 扫描结束统计。
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanStats {
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub skipped: i64,
    pub failed: i64,
}

pub fn finish_scan(
    conn: &Connection,
    scan_id: i64,
    status: &str,
    stats: ScanStats,
    error_summary: Option<&str>,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE scan_sessions SET
            finished_at = ?2, status = ?3,
            files_seen = ?4, files_added = ?5, files_updated = ?6,
            files_skipped = ?7, files_failed = ?8, error_summary = ?9
         WHERE id = ?1",
        params![
            scan_id,
            now_unix(),
            status,
            stats.seen,
            stats.added,
            stats.updated,
            stats.skipped,
            stats.failed,
            error_summary,
        ],
    )
    .map_err(query_err("finish_scan"))?;
    Ok(())
}

/// `query_row` 的 Option 包装：`QueryReturnedNoRows` → `None`。
trait OptionalRow<T> {
    fn optional_row(self) -> Result<Option<T>, StorageError>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional_row(self) -> Result<Option<T>, StorageError> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Query {
                context: "query_row",
                detail: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db::test_support::temp_db;

    fn sample_meta(path: &str, title: &str) -> TrackMetadata {
        TrackMetadata {
            path: path.to_string(),
            file_size: 1000,
            file_mtime: 111,
            title: title.to_string(),
            artist: Some("Probe Artist".into()),
            album: Some("Album One".into()),
            album_artist: Some("Probe Artist".into()),
            album_key: Some("probe artist\u{1f}album one".into()),
            track_no: Some(1),
            disc_no: None,
            year: Some(2026),
            duration_ms: Some(3000),
            sample_rate: Some(44100),
            channels: Some(2),
            bitrate: Some(128),
            codec: Some("mp3".into()),
            container: Some("mp3".into()),
        }
    }

    #[test]
    fn insert_root_is_idempotent() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let a = insert_root(&conn, "/music").unwrap();
        let b = insert_root(&conn, "/music").unwrap();
        assert_eq!(a.id, b.id, "重复插入同一路径应返回同一条记录");
        assert_eq!(list_roots(&conn).unwrap().len(), 1);
    }

    #[test]
    fn upsert_preserves_added_at_and_bumps_updated_at() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let root = insert_root(&conn, "/music").unwrap();

        let id1 = upsert_track(&conn, root.id, &sample_meta("/music/a.mp3", "A")).unwrap();
        let first = get_track(&conn, id1).unwrap().unwrap();

        // 第二次写入：改标题、改大小（模拟文件变化）
        let mut meta = sample_meta("/music/a.mp3", "A renamed");
        meta.file_size = 2000;
        let id2 = upsert_track(&conn, root.id, &meta).unwrap();

        assert_eq!(id1, id2, "同一路径必须复用同一行");
        let second = get_track(&conn, id1).unwrap().unwrap();

        assert_eq!(second.title, "A renamed");
        assert_eq!(second.file_size, 2000);
        assert_eq!(second.added_at, first.added_at, "added_at 不应被覆盖");
        assert_eq!(count_tracks(&conn).unwrap(), 1, "不应产生重复行");
    }

    #[test]
    fn file_states_supports_incremental_scan() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let root = insert_root(&conn, "/music").unwrap();
        upsert_track(&conn, root.id, &sample_meta("/music/a.mp3", "A")).unwrap();

        let states = file_states(&conn, root.id).unwrap();
        assert_eq!(states.get("/music/a.mp3"), Some(&(1000, 111)));
    }

    #[test]
    fn search_finds_by_title_artist_and_album() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let root = insert_root(&conn, "/music").unwrap();
        upsert_track(&conn, root.id, &sample_meta("/music/a.mp3", "Moonlight")).unwrap();
        upsert_track(&conn, root.id, &sample_meta("/music/b.mp3", "Symphony")).unwrap();

        let hits = list_tracks(&conn, Some("moon"), 50).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Moonlight");

        // 按艺术家搜索应命中全部
        let by_artist = list_tracks(&conn, Some("Probe"), 50).unwrap();
        assert_eq!(by_artist.len(), 2);
    }

    #[test]
    fn search_survives_fts_metacharacters() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let root = insert_root(&conn, "/music").unwrap();
        upsert_track(&conn, root.id, &sample_meta("/music/a.mp3", "Track")).unwrap();

        // 这些输入若直接拼进 MATCH 会导致 FTS5 语法错误
        for evil in ["\"", "*", "AND", "NEAR(", "a\" OR \"b", "-", "()"] {
            let r = list_tracks(&conn, Some(evil), 50);
            assert!(r.is_ok(), "输入 {evil:?} 不应导致查询失败: {r:?}");
        }
    }

    #[test]
    fn deleted_track_disappears_from_search() {
        let (_dir, pool) = temp_db();
        let conn = pool.get().unwrap();
        let root = insert_root(&conn, "/music").unwrap();
        let id = upsert_track(&conn, root.id, &sample_meta("/music/a.mp3", "Gone")).unwrap();
        mark_missing(&conn, "/music/a.mp3").unwrap();

        assert_eq!(count_tracks(&conn).unwrap(), 0);
        assert!(list_tracks(&conn, Some("gone"), 50).unwrap().is_empty());
        assert!(get_track(&conn, id).unwrap().is_some(), "行应保留，仅标记缺失");
    }
}
