//! 播放相关命令。
//!
//! 全部经 `blocking()`：`load_and_play` 会打开文件与创建解码器，
//! 属于阻塞工作，不得占用异步运行时线程（NFR-1.7）。

use tauri::State;

use crate::app::library_service;
use crate::app::{AppState, PlaybackEvent, PlaybackSnapshot};
use crate::domain::ErrorPayload;

/// 用给定曲目整体替换队列，并从 `start_index` 开始播放。
///
/// 前端传 `{ trackIds: [...], startIndex: n }`（Tauri 命令参数为 camelCase）。
#[tauri::command]
pub async fn playback_play_tracks(
    state: State<'_, AppState>,
    track_ids: Vec<i64>,
    start_index: usize,
) -> Result<(), ErrorPayload> {
    let pool = state.pool.clone();
    let playback = state.playback.clone();

    super::blocking(move || {
        if track_ids.is_empty() {
            return Err(crate::domain::AppError::Playback(
                crate::domain::PlaybackError::EmptyQueue,
            ));
        }
        let entries = library_service::load_queue_entries(&pool, &track_ids)?;
        playback.play_tracks(entries, start_index)?;
        Ok(())
    })
    .await
}

/// 播放 / 暂停切换。
#[tauri::command]
pub async fn playback_toggle(state: State<'_, AppState>) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.toggle()).await
}

#[tauri::command]
pub async fn playback_stop(state: State<'_, AppState>) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.stop()).await
}

#[tauri::command]
pub async fn playback_next(
    state: State<'_, AppState>,
) -> Result<Option<PlaybackEvent>, ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.next()).await
}

#[tauri::command]
pub async fn playback_previous(
    state: State<'_, AppState>,
) -> Result<Option<PlaybackEvent>, ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.previous()).await
}

#[tauri::command]
pub async fn playback_seek(
    state: State<'_, AppState>,
    position_ms: i64,
) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.seek(position_ms)).await
}

#[tauri::command]
pub async fn playback_set_volume(
    state: State<'_, AppState>,
    volume: f32,
) -> Result<(), ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || playback.set_volume(volume)).await
}

#[tauri::command]
pub async fn playback_get_state(state: State<'_, AppState>) -> Result<PlaybackSnapshot, ErrorPayload> {
    let playback = state.playback.clone();
    super::blocking(move || Ok(playback.snapshot())).await
}
