/**
 * 与 Rust 侧 DTO 一一对应的类型。
 *
 * Rust 侧所有 `#[derive(Serialize)]` 类型都带 `#[serde(rename_all = "camelCase")]`，
 * 因此这里的字段名使用 camelCase。
 *
 * ⚠️ 任何一侧的字段变更都必须同步修改另一侧 —— 这是当前手写契约的代价，
 * 已记录在 docs/architecture.md 的 Q-1 中（是否引入自动生成）。
 */

export interface LibraryRoot {
  id: number;
  path: string;
  enabled: boolean;
  lastScanAt: number | null;
  createdAt: number;
}

export interface Track {
  id: number;
  path: string;

  title: string;
  artist: string | null;
  album: string | null;
  albumArtist: string | null;
  albumKey: string | null;
  trackNo: number | null;
  discNo: number | null;
  year: number | null;

  durationMs: number | null;
  sampleRate: number | null;
  channels: number | null;
  bitrate: number | null;
  codec: string | null;
  container: string | null;

  fileSize: number;
  fileMtime: number;
  addedAt: number;
  updatedAt: number;
}

export interface FileError {
  name: string;
  reason: string;
}

export interface ScanSummary {
  rootId: number;
  seen: number;
  added: number;
  updated: number;
  skipped: number;
  failed: number;
  missing: number;
  durationMs: number;
  errors: FileError[];
}

export interface LibraryStats {
  rootCount: number;
  trackCount: number;
}

/** Rust `ErrorPayload` 的镜像。 */
export interface ErrorPayload {
  code: string;
  message: string;
  retryable: boolean;
}

// ══════════════════════════════════════════════════════════════
// 播放
// ══════════════════════════════════════════════════════════════

export type TransportStatus =
  | "idle"
  | "playing"
  | "paused"
  | "stopped"
  | "error";

/**
 * 循环模式。
 *
 * 与 Rust `RepeatMode` 一一对应（`#[serde(rename_all = "lowercase")]`）。
 *
 * - `off` —— 顺序播放，队列播完即停
 * - `all` —— 列表循环，末曲之后回到首曲
 * - `one` —— 单曲循环，自然结束后重播同一首（手动"下一首"仍然前进）
 */
export type RepeatMode = "off" | "all" | "one";

/** 最近一次曲目级失败。 */
export interface TrackFailure {
  trackId: number;
  code: string;
  message: string;
}

/**
 * 播放状态快照。
 *
 * 字段名与 Rust `PlaybackSnapshot` 一一对应，
 * 由 `src-tauri/tests/ipc_contract.rs` 锁定。
 */
export interface PlaybackSnapshot {
  status: TransportStatus;
  trackId: number | null;
  title: string | null;
  artist: string | null;
  album: string | null;
  durationMs: number | null;
  positionMs: number;
  volume: number;
  queueLength: number;
  queueIndex: number | null;
  hasNext: boolean;
  hasPrevious: boolean;
  /** 循环模式 */
  repeat: RepeatMode;
  /** 随机播放开关。置换顺序由 Rust 持有，不跨 IPC */
  shuffle: boolean;
  /** 音频引擎描述，用于诊断 */
  engine: string;
  lastFailure: TrackFailure | null;
}

/**
 * 离散播放事件。
 *
 * ⚠️ Rust 侧使用 `#[serde(tag = "kind", rename_all = "camelCase",
 * rename_all_fields = "camelCase")]` —— 变体名与**字段名**都是 camelCase。
 */
export type PlaybackEvent =
  | { kind: "trackStarted"; trackId: number }
  | { kind: "trackEnded"; trackId: number }
  | { kind: "trackFailed"; trackId: number; code: string; message: string }
  | { kind: "queueFinished" }
  | { kind: "deviceLost"; reason: string };
