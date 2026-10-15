//! SQLite 数据访问层。
//!
//! 选型 `rusqlite` + `bundled`（见 docs/architecture.md §2.11）：
//! SQLite 编进二进制，无系统依赖，FTS5 开箱可用。
//!
//! ⚠️ rusqlite 是**同步**的。所有调用必须在阻塞线程池中执行，
//! 不得直接在异步运行时线程上跑（NFR-1.7）。

pub mod repo;
pub mod schema;

use std::path::Path;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::domain::StorageError;

pub type DbPool = Pool<SqliteConnectionManager>;
pub type DbConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// 每个连接建立时统一设置的 PRAGMA。
const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store   = MEMORY;
";

/// 打开（必要时创建）数据库并完成迁移。
pub fn open(path: &Path) -> Result<DbPool, StorageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StorageError::PoolInit {
            detail: format!("创建数据目录失败: {e}"),
        })?;
    }

    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        c.execute_batch(PRAGMAS)?;
        Ok(())
    });

    let pool = Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| StorageError::PoolInit {
            detail: e.to_string(),
        })?;

    {
        let mut conn = pool.get().map_err(|e| StorageError::PoolInit {
            detail: e.to_string(),
        })?;
        migrate(&mut conn)?;
    }

    Ok(pool)
}

/// 按 `user_version` 顺序执行迁移。每个版本在单事务内完成。
pub fn migrate(conn: &mut Connection) -> Result<(), StorageError> {
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| StorageError::Query {
            context: "read user_version",
            detail: e.to_string(),
        })?;

    if current > schema::TARGET_VERSION {
        return Err(StorageError::SchemaTooNew {
            db_version: current,
            app_version: schema::TARGET_VERSION,
        });
    }

    for m in schema::MIGRATIONS {
        if m.version <= current {
            continue;
        }
        let tx = conn.transaction().map_err(|e| StorageError::Migration {
            from: current,
            to: m.version,
            detail: e.to_string(),
        })?;

        tx.execute_batch(m.sql).map_err(|e| StorageError::Migration {
            from: current,
            to: m.version,
            detail: e.to_string(),
        })?;

        // PRAGMA 不支持参数绑定，此处版本号来自本模块常量，非外部输入
        tx.pragma_update(None, "user_version", m.version)
            .map_err(|e| StorageError::Migration {
                from: current,
                to: m.version,
                detail: e.to_string(),
            })?;

        tx.commit().map_err(|e| StorageError::Migration {
            from: current,
            to: m.version,
            detail: e.to_string(),
        })?;
    }

    Ok(())
}

/// 应用数据目录（遵循 XDG，不硬编码 ~/.config 等路径）。
pub fn data_dir() -> Result<std::path::PathBuf, StorageError> {
    let dirs = directories::ProjectDirs::from("", "", "aurora-music").ok_or_else(|| {
        StorageError::PoolInit {
            detail: "无法解析用户数据目录".into(),
        }
    })?;
    Ok(dirs.data_dir().to_path_buf())
}

/// 默认数据库文件路径。
pub fn default_db_path() -> Result<std::path::PathBuf, StorageError> {
    Ok(data_dir()?.join("aurora.db"))
}

/// 当前 Unix 时间戳（秒）。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use tempfile::TempDir;

    /// 建立一个隔离的临时数据库，用于集成测试。
    ///
    /// 使用真实文件而非 `:memory:`，这样 WAL 模式、外键、迁移
    /// 都走与生产完全相同的代码路径。
    pub fn temp_db() -> (TempDir, DbPool) {
        let dir = TempDir::new().expect("创建临时目录");
        let path = dir.path().join("test.db");
        let pool = open(&path).expect("打开测试数据库");
        (dir, pool)
    }
}
