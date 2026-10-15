import { useCallback } from "react";

import type { Track } from "../ipc/types";
import { formatDuration } from "../lib/format";

interface Props {
  tracks: Track[];
  /** 当前正在播放的曲目 ID（来自 Rust 状态快照） */
  playingTrackId: number | null;
  isPlaying: boolean;
  /** 点击某一行：以整个列表为队列，从该下标开始播放 */
  onPlay: (trackIds: number[], startIndex: number) => void;
}

export function TrackList({ tracks, playingTrackId, isPlaying, onPlay }: Props) {
  // 队列 = 当前显示的全部曲目（搜索结果同样适用）
  const allIds = tracks.map((t) => t.id);

  return (
    <div role="table" aria-label="曲目列表">
      <div className="list-header" role="row">
        <span className="col-index" role="columnheader">
          #
        </span>
        <span className="col-title" role="columnheader">
          标题
        </span>
        <span className="col-album" role="columnheader">
          专辑
        </span>
        <span className="col-codec" role="columnheader">
          格式
        </span>
        <span className="col-duration" role="columnheader">
          时长
        </span>
      </div>

      <div role="rowgroup">
        {tracks.map((track, i) => (
          <TrackRow
            key={track.id}
            track={track}
            index={i}
            isCurrent={track.id === playingTrackId}
            isPlaying={isPlaying}
            onActivate={() => onPlay(allIds, i)}
          />
        ))}
      </div>
    </div>
  );
}

function TrackRow({
  track,
  index,
  isCurrent,
  isPlaying,
  onActivate,
}: {
  track: Track;
  index: number;
  isCurrent: boolean;
  isPlaying: boolean;
  onActivate: () => void;
}) {
  const artist = track.artist ?? "未知艺术家";
  const album = track.album ?? "—";

  const handleKey = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
    [onActivate],
  );

  return (
    <div
      className={`track-row${isCurrent ? " current" : ""}`}
      role="row"
      tabIndex={0}
      onClick={onActivate}
      onKeyDown={handleKey}
      title={`${track.path}\n点击播放`}
      aria-label={`${track.title}，${artist}${isCurrent ? "，正在播放" : ""}`}
      aria-current={isCurrent ? "true" : undefined}
    >
      <span className="track-index" role="cell">
        {isCurrent ? (
          <span className={`playing-indicator${isPlaying ? " animating" : ""}`} aria-hidden="true">
            {isPlaying ? "♫" : "⏸"}
          </span>
        ) : (
          index + 1
        )}
      </span>

      <div className="track-main" role="cell">
        <span className="track-title">{track.title}</span>
        <span className="track-artist">{artist}</span>
      </div>

      <span className="track-album" role="cell" title={track.album ?? undefined}>
        {album}
      </span>

      <span className="track-codec" role="cell">
        {track.codec ?? "—"}
      </span>

      <span className="track-duration" role="cell">
        {formatDuration(track.durationMs)}
      </span>
    </div>
  );
}
