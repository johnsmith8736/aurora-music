/**
 * 播放状态容器。
 *
 * # 设计要点
 *
 * **Rust 是播放状态的唯一真相源**，这里只是它的镜像。
 * 控制入口有三个（界面按钮、键盘、将来的 MPRIS），前端各自维护状态
 * 必然导致三方差分。
 *
 * # 关于进度条的平滑
 *
 * 后端以约 5 Hz 推送权威位置。这里**不做** requestAnimationFrame 插值 ——
 * 那会导致每秒 60 次整树重渲染。改用 CSS `transition: width 200ms linear`
 * 在两个权威位置之间做视觉补间：同样的平滑观感，零额外渲染开销。
 * 暂停时由 CSS 类关闭过渡，避免拖动后缓慢滑回。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import {
  AppError,
  onPlaybackEvent,
  onPlaybackState,
  playback,
  queue,
} from "../ipc/client";
import type { PlaybackEvent, PlaybackSnapshot, RepeatMode } from "../ipc/types";

export interface PlayerController {
  snapshot: PlaybackSnapshot | null;
  /** 最近一次离散事件，供界面提示 */
  lastEvent: PlaybackEvent | null;
  error: string | null;
  clearError: () => void;
  toggle: () => Promise<void>;
  stop: () => Promise<void>;
  next: () => Promise<void>;
  previous: () => Promise<void>;
  seek: (positionMs: number) => Promise<void>;
  setVolume: (volume: number) => Promise<void>;
  playTracks: (trackIds: number[], startIndex: number) => Promise<void>;
  /** 设置循环模式。模式本身不在这里保存 —— 由 Rust 持有并经快照回传。 */
  setRepeat: (mode: RepeatMode) => Promise<void>;
  /** 开关随机播放。 */
  setShuffle: (enabled: boolean) => Promise<void>;
  playingTrackId: number | null;
}

export function usePlayer(): PlayerController {
  const [snapshot, setSnapshot] = useState<PlaybackSnapshot | null>(null);
  const [lastEvent, setLastEvent] = useState<PlaybackEvent | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 卸载后不再 setState，避免"更新已卸载组件"警告
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    const pending: Promise<UnlistenFn>[] = [];

    pending.push(
      onPlaybackState((s) => {
        if (aliveRef.current) setSnapshot(s);
      }),
    );
    pending.push(
      onPlaybackEvent((e) => {
        if (aliveRef.current) setLastEvent(e);
      }),
    );

    // 主动拉一次初始状态：否则要等到第一次变化界面才有数据
    void (async () => {
      try {
        const initial = await playback.getState();
        if (aliveRef.current) setSnapshot(initial);
      } catch {
        // 后端尚未就绪时静默；后续事件会补上
      }
    })();

    return () => {
      aliveRef.current = false;
      void Promise.all(pending).then((fns) => fns.forEach((f) => f()));
    };
  }, []);

  /** 统一把命令错误收敛成可显示的消息。 */
  const run = useCallback(async (fn: () => Promise<unknown>) => {
    try {
      await fn();
      setError(null);
    } catch (e) {
      setError(e instanceof AppError ? e.message : "播放操作失败。");
    }
  }, []);

  const toggle = useCallback(() => run(() => playback.toggle()), [run]);
  const stop = useCallback(() => run(() => playback.stop()), [run]);
  const next = useCallback(() => run(() => playback.next()), [run]);
  const previous = useCallback(() => run(() => playback.previous()), [run]);

  const seek = useCallback(
    (positionMs: number) => run(() => playback.seek(positionMs)),
    [run],
  );

  const setVolume = useCallback(
    (volume: number) => run(() => playback.setVolume(volume)),
    [run],
  );

  const playTracks = useCallback(
    (trackIds: number[], startIndex: number) =>
      run(() => playback.playTracks(trackIds, startIndex)),
    [run],
  );

  const setRepeat = useCallback(
    (mode: RepeatMode) => run(() => queue.setRepeat(mode)),
    [run],
  );

  const setShuffle = useCallback(
    (enabled: boolean) => run(() => queue.setShuffle(enabled)),
    [run],
  );

  return {
    snapshot,
    lastEvent,
    error,
    clearError: useCallback(() => setError(null), []),
    toggle,
    stop,
    next,
    previous,
    seek,
    setVolume,
    playTracks,
    setRepeat,
    setShuffle,
    playingTrackId: snapshot?.trackId ?? null,
  };
}
