//! 资料库相关命令。

use tauri::State;

use crate::app::library_service::{self, LibraryStats, ScanSummary};
use crate::app::AppState;
use crate::domain::model::{LibraryRoot, RootId, Track};
use crate::domain::ErrorPayload;

/// 添加一个音乐目录。路径会被规范化为绝对路径。
#[tauri::command]
pub async fn library_add_root(
    state: State<'_, AppState>,
    path: String,
) -> Result<LibraryRoot, ErrorPayload> {
    let pool = state.pool.clone();
    super::blocking(move || library_service::add_root(&pool, &path)).await
}

#[tauri::command]
pub async fn library_list_roots(state: State<'_, AppState>) -> Result<Vec<LibraryRoot>, ErrorPayload> {
    let pool = state.pool.clone();
    super::blocking(move || library_service::list_roots(&pool)).await
}

#[tauri::command]
pub async fn library_remove_root(
    state: State<'_, AppState>,
    root_id: RootId,
) -> Result<(), ErrorPayload> {
    let pool = state.pool.clone();
    super::blocking(move || library_service::remove_root(&pool, root_id)).await
}

/// 扫描根目录。这是同步阻塞操作，大目录可能耗时较长。
#[tauri::command]
pub async fn library_scan_root(
    state: State<'_, AppState>,
    root_id: RootId,
) -> Result<ScanSummary, ErrorPayload> {
    let pool = state.pool.clone();
    super::blocking(move || library_service::scan_root(&pool, root_id)).await
}

/// 列出曲目。`query` 为 `None` 或空串时返回全部。
#[tauri::command]
pub async fn library_list_tracks(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<Track>, ErrorPayload> {
    let pool = state.pool.clone();
    let limit = limit.unwrap_or(1000);
    super::blocking(move || library_service::list_tracks(&pool, query.as_deref(), limit)).await
}

#[tauri::command]
pub async fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats, ErrorPayload> {
    let pool = state.pool.clone();
    super::blocking(move || library_service::stats(&pool)).await
}
