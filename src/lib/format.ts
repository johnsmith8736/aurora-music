/** 展示层格式化工具。纯函数，可单测。 */

/** 毫秒 → `m:ss` 或 `h:mm:ss`。未知时长返回 `--:--`。 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || ms < 0 || !Number.isFinite(ms)) {
    return "--:--";
  }
  const totalSeconds = Math.floor(ms / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);

  const pad = (n: number) => n.toString().padStart(2, "0");

  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}

/** 千分位数字。 */
export function formatCount(n: number): string {
  return n.toLocaleString("zh-CN");
}

/** 字节 → 人类可读。 */
export function formatFileSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || bytes < 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value < 10 && i > 0 ? value.toFixed(1) : Math.round(value)} ${units[i]}`;
}

/** 采样率 → `44.1 kHz`。未知返回 `—`。 */
export function formatSampleRate(hz: number | null | undefined): string {
  if (hz === null || hz === undefined || hz <= 0) return "—";
  return `${(hz / 1000).toFixed(hz % 1000 === 0 ? 0 : 1)} kHz`;
}

/** 比特率 → `1004 kbps`。未知返回 `—`。 */
export function formatBitrate(bps: number | null | undefined): string {
  if (bps === null || bps === undefined || bps <= 0) return "—";
  return `${Math.round(bps / 1000)} kbps`;
}

/** 声道数 → `单声道` / `N 声道`。未知返回 `—`。 */
export function formatChannels(n: number | null | undefined): string {
  if (n === null || n === undefined || n <= 0) return "—";
  return n === 1 ? "单声道" : `${n} 声道`;
}

/**
 * 位深/采样率等技术信息的一行摘要。全为未知时返回空串。
 *
 * 空串而非 `—`：本函数用于"有则显示"的场景，调用方靠空串判断整行是否省略。
 */
export function formatTechSummary(track: {
  codec: string | null;
  sampleRate: number | null;
  channels: number | null;
  bitrate: number | null;
}): string {
  const parts: string[] = [];
  if (track.codec) parts.push(track.codec.toUpperCase());
  if (track.sampleRate) parts.push(formatSampleRate(track.sampleRate));
  if (track.channels) parts.push(formatChannels(track.channels));
  if (track.bitrate) parts.push(formatBitrate(track.bitrate));
  return parts.join(" · ");
}

/**
 * Unix **毫秒** 时间戳 → 本地 `YYYY-MM-DD HH:mm`。（内部函数，不导出）
 *
 * ⚠️ 刻意不导出：`Track` 里的时间戳**混用两种单位**，导出通用函数必然有
 * 一方被传错。调用方只能二选一，见下面两个具名函数。
 * `<= 0` 视为未知 —— 后端两处时间源失败时都以 `0` 兜底。
 */
function formatDateTime(unixMs: number | null | undefined): string {
  if (unixMs === null || unixMs === undefined || !Number.isFinite(unixMs) || unixMs <= 0) {
    return "—";
  }
  const d = new Date(unixMs);
  if (Number.isNaN(d.getTime())) return "—";
  const pad = (n: number) => n.toString().padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

/**
 * Unix **秒** → 本地日期时间。
 *
 * 用于 `addedAt` / `updatedAt`：后端 `infra/time.rs` 的 `now_unix()` 取的是
 * `as_secs()`，与 `repo::upsert_track` 写入 `added_at` / `updated_at` 的是同一来源。
 */
export function formatDateTimeSec(unixSec: number | null | undefined): string {
  if (unixSec === null || unixSec === undefined) return "—";
  return formatDateTime(unixSec * 1000);
}

/**
 * Unix **毫秒** → 本地日期时间。
 *
 * 用于 `fileMtime`：后端 `infra/scan/mod.rs` 的 `mtime_millis()` 取其
 * `as_millis()`，函数名与文档注释都明写「Unix 毫秒」。
 */
export function formatDateTimeMs(unixMs: number | null | undefined): string {
  return formatDateTime(unixMs);
}
