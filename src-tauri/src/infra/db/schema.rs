//! 数据库 schema 与迁移。
//!
//! 迁移由 SQLite 的 `user_version` 驱动，每个版本在单事务内执行。
//! 新增字段一律通过**新增迁移**完成，绝不修改已发布的迁移。

/// 单个迁移步骤。
pub struct Migration {
    /// 目标版本号（执行后 `user_version` 即为该值）。
    pub version: u32,
    /// 该版本的 SQL。在单事务内执行，失败则整体回滚。
    pub sql: &'static str,
}

/// 本程序期望的 schema 版本。
pub const TARGET_VERSION: u32 = 1;

/// 有序迁移列表。
///
/// ⚠️ 已发布的迁移**不得修改**，只能追加新版本。
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: V1,
}];

/// v1：本地资料库基础（根目录 + 曲目 + 扫描会话 + 全文索引）。
///
/// 歌单 / 收藏 / 播放历史 / 播放队列将在后续里程碑以 v2、v3 追加，
/// 避免在此阶段写入尚未使用的表结构。
const V1: &str = r#"
CREATE TABLE library_roots (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL UNIQUE,
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_scan_at INTEGER,
    created_at   INTEGER NOT NULL
);

CREATE TABLE tracks (
    id           INTEGER PRIMARY KEY,
    root_id      INTEGER REFERENCES library_roots(id) ON DELETE CASCADE,
    path         TEXT    NOT NULL UNIQUE,

    title        TEXT    NOT NULL,
    artist       TEXT,
    album        TEXT,
    album_artist TEXT,
    album_key    TEXT,
    track_no     INTEGER,
    disc_no      INTEGER,
    year         INTEGER,

    duration_ms  INTEGER,
    sample_rate  INTEGER,
    channels     INTEGER,
    bitrate      INTEGER,
    codec        TEXT,
    container    TEXT,

    file_size    INTEGER NOT NULL,
    file_mtime   INTEGER NOT NULL,

    missing_since INTEGER,
    added_at     INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE INDEX idx_tracks_album_key ON tracks(album_key);
CREATE INDEX idx_tracks_artist    ON tracks(artist);
CREATE INDEX idx_tracks_root      ON tracks(root_id);
CREATE INDEX idx_tracks_missing   ON tracks(missing_since)
    WHERE missing_since IS NOT NULL;

CREATE TABLE scan_sessions (
    id            INTEGER PRIMARY KEY,
    root_id       INTEGER NOT NULL REFERENCES library_roots(id) ON DELETE CASCADE,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    status        TEXT    NOT NULL DEFAULT 'running',
    files_seen    INTEGER NOT NULL DEFAULT 0,
    files_added   INTEGER NOT NULL DEFAULT 0,
    files_updated INTEGER NOT NULL DEFAULT 0,
    files_skipped INTEGER NOT NULL DEFAULT 0,
    files_failed  INTEGER NOT NULL DEFAULT 0,
    error_summary TEXT
);

CREATE INDEX idx_scan_sessions_root ON scan_sessions(root_id, started_at DESC);

-- 全文索引：外部内容表，由触发器与 tracks 保持同步
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, album_artist,
    content='tracks',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER tracks_fts_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album, album_artist)
    VALUES (new.id, new.title, new.artist, new.album, new.album_artist);
END;

CREATE TRIGGER tracks_fts_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, album_artist)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.album_artist);
END;

CREATE TRIGGER tracks_fts_au AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, album_artist)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.album_artist);
    INSERT INTO tracks_fts(rowid, title, artist, album, album_artist)
    VALUES (new.id, new.title, new.artist, new.album, new.album_artist);
END;
"#;
