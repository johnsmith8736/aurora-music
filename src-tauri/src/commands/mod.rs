//! 命令层：Tauri IPC 的薄适配器。
//!
//! 职责仅三件：接收参数 → 转交服务层 → 把错误序列化。
//! **不写业务逻辑**，不直接触碰 DB / 文件系统。
//!
//! ⚠️ 所有数据库操作都是阻塞的，必须经 `blocking()` 放进阻塞线程池，
//! 否则会卡住异步运行时（NFR-1.7）。

pub mod library;
pub mod playback;

use crate::domain::{AppError, ErrorPayload};

/// 在阻塞线程池中执行同步工作，并把错误转换为可跨 IPC 的载荷。
pub async fn blocking<T, F>(f: F) -> Result<T, ErrorPayload>
where
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.into()),
        Err(join_err) => Err(AppError::Internal {
            detail: format!("后台任务失败: {join_err}"),
        }
        .into()),
    }
}
