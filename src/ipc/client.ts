/**
 * 前端访问后端的**唯一**入口。
 *
 * ⚠️ 全项目只有本文件允许出现 `invoke()`。
 * 组件一律通过 `api.*` 调用，这样错误归一化、日志、类型约束
 * 都只有一个收口点（docs/architecture.md §4.1）。
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import type {
  ErrorPayload,
  LibraryRoot,
  LibraryStats,
  PlaybackEvent,
  PlaybackSnapshot,
  RepeatMode,
  ScanSummary,
  Track,
} from "./types";

/** 归一化后的错误。组件只需读 `code` / `message` / `retryable`。 */
export class AppError extends Error {
  readonly code: string;
  readonly retryable: boolean;

  constructor(payload: ErrorPayload) {
    super(payload.message);
    this.name = "AppError";
    this.code = payload.code;
    this.retryable = payload.retryable;
  }
}

function isErrorPayload(v: unknown): v is ErrorPayload {
  return (
    typeof v === "object" &&
    v !== null &&
    "code" in v &&
    "message" in v &&
    typeof (v as ErrorPayload).code === "string"
  );
}

/**
 * 把任意抛出物归一化为 AppError。
 *
 * Rust 侧命令返回 `Err(ErrorPayload)` 时，Tauri 会把该对象原样送到
 * JS 的 catch 分支，因此这里通常拿到的就是 ErrorPayload。但插件错误、
 * IPC 层错误可能是普通字符串，必须兜底。
 */
function normalizeError(raw: unknown): AppError {
  if (isErrorPayload(raw)) {
    return new AppError(raw);
  }
  const message =
    typeof raw === "string" ? raw : raw instanceof Error ? raw.message : "未知错误";
  return new AppError({ code: "IPC_UNKNOWN", message, retryable: false });
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (raw) {
    throw normalizeError(raw);
  }
}

export const api = {
  // ── 资料库根目录 ────────────────────────────────────────────
  listRoots: () => call<LibraryRoot[]>("library_list_roots"),

  addRoot: (path: string) => call<LibraryRoot>("library_add_root", { path }),

  removeRoot: (rootId: number) =>
    call<void>("library_remove_root", { rootId }),

  /**
   * 扫描根目录。这是同步阻塞命令，大目录可能耗时数秒到数分钟，
   * 调用方必须显示进度反馈。
   */
  scanRoot: (rootId: number) =>
    call<ScanSummary>("library_scan_root", { rootId }),

  // ── 曲目 ────────────────────────────────────────────────────
  listTracks: (query?: string, limit?: number) =>
    call<Track[]>("library_list_tracks", {
      query: query && query.trim() !== "" ? query : null,
      limit: limit ?? null,
    }),

  /**
   * 按 ID 取单曲完整元数据，供"正在播放"界面使用。
   * 曲目不存在时抛 `AppError`，`code === "LIBRARY_TRACK_NOT_FOUND"`。
   */
  getTrack: (trackId: number) => call<Track>("library_get_track", { trackId }),

  stats: () => call<LibraryStats>("library_stats"),
};

/** 打开系统目录选择器。取消时返回 null。 */
export async function pickMusicFolder(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择音乐目录",
  });
  if (selected === null) return null;
  return Array.isArray(selected) ? (selected[0] ?? null) : selected;
}

// ══════════════════════════════════════════════════════════════
// 播放
// ══════════════════════════════════════════════════════════════

export const playback = {
  /**
   * 用给定曲目整体替换播放队列，并从 `startIndex` 开始播放。
   * `startIndex` 是 `trackIds` 中的下标，不是曲目 ID。
   */
  playTracks: (trackIds: number[], startIndex: number) =>
    call<void>("playback_play_tracks", { trackIds, startIndex }),

  toggle: () => call<void>("playback_toggle"),

  stop: () => call<void>("playback_stop"),

  /** 返回新开始的曲目事件；已在末曲时返回 null。 */
  next: () => call<PlaybackEvent | null>("playback_next"),

  previous: () => call<PlaybackEvent | null>("playback_previous"),

  seek: (positionMs: number) =>
    call<void>("playback_seek", { positionMs: Math.max(0, Math.round(positionMs)) }),

  setVolume: (volume: number) =>
    call<void>("playback_set_volume", { volume: Math.min(1, Math.max(0, volume)) }),

  getState: () => call<PlaybackSnapshot>("playback_get_state"),
};

// ══════════════════════════════════════════════════════════════
// 队列（播放模式）
// ══════════════════════════════════════════════════════════════

/**
 * 播放模式只有 setter —— 状态由 Rust 持有，读回统一走 `playback.getState()`。
 * 这样"Rust 是唯一真相源"不会因为多了个读接口而被绕过。
 */
export const queue = {
  setRepeat: (mode: RepeatMode) => call<void>("queue_set_repeat", { mode }),

  /** 开启时会生成一份固定置换（FR-3.5），不打断当前曲目。 */
  setShuffle: (enabled: boolean) =>
    call<void>("queue_set_shuffle", { enabled }),
};

// ══════════════════════════════════════════════════════════════
// 后端事件订阅
// ══════════════════════════════════════════════════════════════

/**
 * 订阅播放状态广播。
 *
 * 后端在状态**发生变化**时才推送（空闲时零流量），
 * 播放中约 5 Hz，用于驱动进度条。
 */
export function onPlaybackState(
  handler: (snapshot: PlaybackSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<PlaybackSnapshot>("playback:state", (e) => handler(e.payload));
}

/** 订阅离散播放事件（曲目切换、失败、设备丢失等）。 */
export function onPlaybackEvent(
  handler: (event: PlaybackEvent) => void,
): Promise<UnlistenFn> {
  return listen<PlaybackEvent>("playback:event", (e) => handler(e.payload));
}
