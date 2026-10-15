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

/** 位深/采样率等技术信息的一行摘要。 */
export function formatTechSummary(track: {
  codec: string | null;
  sampleRate: number | null;
  channels: number | null;
  bitrate: number | null;
}): string {
  const parts: string[] = [];
  if (track.codec) parts.push(track.codec.toUpperCase());
  if (track.sampleRate) {
    parts.push(`${(track.sampleRate / 1000).toFixed(track.sampleRate % 1000 === 0 ? 0 : 1)} kHz`);
  }
  if (track.channels) parts.push(track.channels === 1 ? "单声道" : `${track.channels} 声道`);
  if (track.bitrate) parts.push(`${Math.round(track.bitrate / 1000)} kbps`);
  return parts.join(" · ");
}
