import { useCallback, useRef } from "react";

import { formatDuration } from "../lib/format";
import type { PlayerController } from "../state/player";

export function PlayerBar({ player }: { player: PlayerController }) {
  const { snapshot, error, clearError } = player;

  const status = snapshot?.status ?? "idle";
  const isPlaying = status === "playing";
  const hasTrack = snapshot?.trackId != null;
  const duration = snapshot?.durationMs ?? 0;
  const position = snapshot?.positionMs ?? 0;

  const progressRef = useRef<HTMLDivElement>(null);

  /** 点击进度条跳转。 */
  const handleSeek = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const el = progressRef.current;
      if (!el || duration <= 0) return;
      const rect = el.getBoundingClientRect();
      const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
      void player.seek(Math.round(ratio * duration));
    },
    [duration, player],
  );

  /** 键盘可达：左右方向键 ±5 秒。 */
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

  const ratio = duration > 0 ? Math.min(1, position / duration) : 0;

  return (
    <footer className="playerbar" aria-label="播放控制">
      {/* ── 当前曲目 ─────────────────────────────────────── */}
      <div className="player-now">
        <div className="player-art" aria-hidden="true">
          {isPlaying ? "♫" : "♪"}
        </div>
        <div className="player-meta">
          <span className={`player-title${hasTrack ? "" : " dim"}`}>
            {snapshot?.title ?? "未在播放"}
          </span>
          <span className="player-sub">
            {hasTrack
              ? (snapshot?.artist ?? "未知艺术家")
              : "从资料库中选择一首曲目开始播放"}
          </span>
        </div>
      </div>

      {/* ── 传输控制 ─────────────────────────────────────── */}
      <div className="player-center">
        <div className="player-controls">
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
            disabled={!hasTrack}
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
            disabled={!hasTrack}
            aria-label="停止"
            title="停止并清空队列"
          >
            ⏹
          </button>
        </div>

        <div className="player-progress">
          <span className="time">{formatDuration(hasTrack ? position : null)}</span>

          <div
            ref={progressRef}
            className={`progress-track${hasTrack ? "" : " disabled"}`}
            onClick={handleSeek}
            onKeyDown={handleSeekKey}
            role="slider"
            tabIndex={hasTrack ? 0 : -1}
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

          <span className="time">{formatDuration(hasTrack ? duration : null)}</span>
        </div>
      </div>

      {/* ── 右侧：音量与状态 ─────────────────────────────── */}
      <div className="player-right">
        {(error || snapshot?.lastFailure) && (
          <button
            className="player-alert"
            onClick={clearError}
            title="点击关闭提示"
            aria-label="播放错误提示，点击关闭"
          >
            ⚠ {error ?? snapshot?.lastFailure?.message}
          </button>
        )}

        <span className="volume-icon" aria-hidden="true">
          {snapshot && snapshot.volume === 0 ? "🔇" : "🔊"}
        </span>
        <input
          className="volume-slider"
          type="range"
          min={0}
          max={1}
          step={0.01}
          value={snapshot?.volume ?? 1}
          onChange={(e) => void player.setVolume(Number(e.currentTarget.value))}
          aria-label="音量"
          title={`音量 ${Math.round((snapshot?.volume ?? 1) * 100)}%`}
        />
        <span className="queue-count" title="队列长度">
          {snapshot && snapshot.queueLength > 0
            ? `${(snapshot.queueIndex ?? 0) + 1}/${snapshot.queueLength}`
            : "—"}
        </span>
      </div>
    </footer>
  );
}
