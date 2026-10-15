import { useCallback, useEffect, useRef, useState } from "react";

import { api } from "../ipc/client";
import type { Track } from "../ipc/types";
import {
  formatBitrate,
  formatChannels,
  formatDateTimeMs,
  formatDateTimeSec,
  formatDuration,
  formatFileSize,
  formatSampleRate,
} from "../lib/format";
import type { PlayerController } from "../state/player";
import { RepeatToggle, ShuffleToggle } from "./PlayModeToggles";

interface Props {
  player: PlayerController;
  onBack: () => void;
}

/**
 * 主内容区的"播放界面"。
 *
 * 数据来源有两条，刻意分开：
 *
 * - **播放状态**（进度、传输状态）来自 `player.snapshot`，随 5 Hz 广播更新
 * - **曲目详情**（编码、采样率、文件路径……）来自一次性的 `api.getTrack()`，
 *   因为 `PlaybackSnapshot` 里没有这些字段（见 `src/ipc/types.ts`）
 *
 * 因此曲目切换时只重新拉一次详情，进度更新不会触发任何额外请求。
 */
export function NowPlaying({ player, onBack }: Props) {
  const { snapshot } = player;
  const trackId = player.playingTrackId;

  const [track, setTrack] = useState<Track | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  // ── 拉取当前曲目的完整元数据 ────────────────────────────────
  useEffect(() => {
    if (trackId === null) {
      setTrack(null);
      setDetailError(null);
      return;
    }

    // 切歌与卸载都会触发 cleanup，用它作废在途响应，避免旧结果覆盖新曲目
    let cancelled = false;

    void (async () => {
      try {
        const fetched = await api.getTrack(trackId);
        if (cancelled) return;
        setTrack(fetched);
        setDetailError(null);
      } catch (err) {
        if (cancelled) return;
        // 详情取不到不算致命：仍用快照里的标题/艺术家把界面渲染出来
        setTrack(null);
        setDetailError(err instanceof Error ? err.message : "读取曲目详情失败");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [trackId]);

  // ── Esc 返回 ────────────────────────────────────────────────
  // 本视图不是模态（没有焦点陷阱、没有遮罩），因此只需一个全局按键监听
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  // ── 进度与跳转（与 PlayerBar 同一套逻辑） ───────────────────
  const duration = snapshot?.durationMs ?? 0;
  const position = snapshot?.positionMs ?? 0;
  const isPlaying = snapshot?.status === "playing";
  const hasTrack = trackId !== null;
  const ratio = duration > 0 ? Math.min(1, position / duration) : 0;

  const progressRef = useRef<HTMLDivElement>(null);

  const handleSeek = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const el = progressRef.current;
      if (!el || duration <= 0) return;
      const rect = el.getBoundingClientRect();
      const r = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
      void player.seek(Math.round(r * duration));
    },
    [duration, player],
  );

  const handleSeekKey = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (duration <= 0) return;
      const step = 5000;
      if (event.key === "ArrowRight") {
        event.preventDefault();
        void player.seek(Math.min(duration, position + step));
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        void player.seek(Math.max(0, position - step));
      }
    },
    [duration, position, player],
  );

  // ── 标题栏（两种情况共用） ──────────────────────────────────
  const header = (
    <header className="nowplaying-head">
      <button
        className="btn btn-ghost"
        onClick={onBack}
        title="返回曲目列表（Esc）"
        aria-label="返回曲目列表"
      >
        ← 返回
      </button>
      <h2 className="section-heading">正在播放</h2>
    </header>
  );

  // ── 无曲目 ──────────────────────────────────────────────────
  if (!hasTrack) {
    return (
      <section className="nowplaying" aria-label="播放界面">
        {header}
        <div className="nowplaying-empty">
          <div className="nowplaying-empty-mark" aria-hidden="true">
            ♪
          </div>
          <p className="nowplaying-empty-text">当前没有正在播放的曲目。</p>
          <p className="nowplaying-empty-hint">从资料库中选择一首曲目开始播放。</p>
        </div>
      </section>
    );
  }

  // 详情尚未到位时退化为快照字段 —— 界面不会空着
  const title = track?.title ?? snapshot?.title ?? "未知曲目";
  const artist = track?.artist ?? snapshot?.artist ?? "未知艺术家";
  const album = track?.album ?? snapshot?.album ?? null;
  const year = track?.year ?? null;

  return (
    <section className="nowplaying" aria-label="播放界面">
      {header}

      <div className="nowplaying-hero">
        {/* FR-7（M3）的封面将填充此块；当前尚无封面能力，不伪造图片 */}
        <div className="nowplaying-art" aria-hidden="true">
          ♪
        </div>

        <div className="nowplaying-info">
          <h1 className="nowplaying-title">{title}</h1>
          <p className="nowplaying-artist">{artist}</p>
          <p className="nowplaying-album">
            {album ?? "未知专辑"}
            {year !== null ? ` · ${year}` : ""}
          </p>
          {detailError !== null && (
            <p className="nowplaying-warn" role="status">
              ⚠ {detailError}
            </p>
          )}
        </div>
      </div>

      <div className="nowplaying-progress">
        <span className="time">{formatDuration(position)}</span>
        <div
          ref={progressRef}
          className="progress-track"
          onClick={handleSeek}
          onKeyDown={handleSeekKey}
          role="slider"
          tabIndex={0}
          aria-label="播放进度"
          aria-valuemin={0}
          aria-valuemax={duration}
          aria-valuenow={Math.round(position)}
          aria-valuetext={`${formatDuration(position)} / ${formatDuration(duration)}`}
        >
          <div
            className={`progress-fill${isPlaying ? " smooth" : ""}`}
            style={{ width: `${ratio * 100}%` }}
          />
        </div>
        <span className="time">{formatDuration(duration)}</span>
      </div>

      <div className="nowplaying-controls">
        <ShuffleToggle player={player} />

        <button
          className="ctrl"
          onClick={() => void player.previous()}
          disabled={!snapshot?.hasPrevious}
          aria-label="上一首"
          title="上一首"
        >
          ⏮
        </button>
        <button
          className="ctrl ctrl-play"
          onClick={() => void player.toggle()}
          aria-label={isPlaying ? "暂停" : "播放"}
          title={isPlaying ? "暂停" : "播放"}
        >
          {isPlaying ? "⏸" : "▶"}
        </button>
        <button
          className="ctrl"
          onClick={() => void player.next()}
          disabled={!snapshot?.hasNext}
          aria-label="下一首"
          title="下一首"
        >
          ⏭
        </button>
        <button
          className="ctrl"
          onClick={() => void player.stop()}
          aria-label="停止"
          title="停止并清空队列"
        >
          ⏹
        </button>

        <RepeatToggle player={player} />
      </div>

      {track !== null && (
        <div className="nowplaying-details">
          <DetailGroup
            heading="曲目信息"
            rows={[
              { label: "专辑", value: track.album ?? "—" },
              { label: "专辑艺术家", value: track.albumArtist ?? "—" },
              { label: "音轨号", value: track.trackNo !== null ? String(track.trackNo) : "—" },
              { label: "碟号", value: track.discNo !== null ? String(track.discNo) : "—" },
              { label: "年份", value: track.year !== null ? String(track.year) : "—" },
            ]}
          />
          <DetailGroup
            heading="音频格式"
            rows={[
              { label: "编码", value: track.codec ?? "—" },
              { label: "容器", value: track.container ?? "—" },
              { label: "比特率", value: formatBitrate(track.bitrate) },
              { label: "采样率", value: formatSampleRate(track.sampleRate) },
              { label: "声道", value: formatChannels(track.channels) },
              { label: "时长", value: formatDuration(track.durationMs) },
            ]}
          />
          <DetailGroup
            heading="文件"
            rows={[
              { label: "路径", value: track.path },
              { label: "体积", value: formatFileSize(track.fileSize) },
              { label: "文件修改时间", value: formatDateTimeMs(track.fileMtime) },
              { label: "入库时间", value: formatDateTimeSec(track.addedAt) },
            ]}
          />
        </div>
      )}
    </section>
  );
}

interface DetailRow {
  label: string;
  value: string;
}

/** 一组「标签 / 值」行。缺失值由调用方统一填 `—`，此处不做过滤。 */
function DetailGroup({ heading, rows }: { heading: string; rows: DetailRow[] }) {
  return (
    <section className="nowplaying-group">
      <h3 className="section-heading">{heading}</h3>
      <dl className="nowplaying-rows">
        {rows.map((row) => (
          <div key={row.label} className="nowplaying-row">
            <dt className="nowplaying-label">{row.label}</dt>
            <dd className="nowplaying-value" title={row.value}>
              {row.value}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
