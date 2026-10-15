import { useCallback, useEffect, useRef, useState } from "react";

import { EmptyState, StatusBanner } from "./components/Feedback";
import { NowPlaying } from "./components/NowPlaying";
import { PlayerBar } from "./components/PlayerBar";
import { Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { TrackList } from "./components/TrackList";
import { api, AppError, pickMusicFolder } from "./ipc/client";
import type { FileError, LibraryRoot, Track } from "./ipc/types";
import { formatCount } from "./lib/format";
import { usePlayer } from "./state/player";

import "./styles/theme.css";
import "./styles/app.css";

/** 搜索防抖时长。太短会频繁打 IPC，太长会显得迟钝。 */
const SEARCH_DEBOUNCE_MS = 220;

interface Notice {
  tone: "error" | "warning" | "success";
  text: string;
  errors?: FileError[];
}

/**
 * 主内容区的当前视图。
 *
 * 这是**前端持有**的状态，不进 Rust 侧 —— `docs/architecture.md` 已约定
 * "当前视图、滚动位置、面板开关"由前端管理。
 */
type View = "library" | "nowPlaying";

export default function App() {
  const [roots, setRoots] = useState<LibraryRoot[]>([]);
  const [tracks, setTracks] = useState<Track[]>([]);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [busyLabel, setBusyLabel] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [booted, setBooted] = useState(false);
  const [view, setView] = useState<View>("library");

  // 播放状态由 Rust 持有，这里只是它的镜像
  const player = usePlayer();

  // 防抖用的最新查询值，避免闭包捕获过期 state
  const searchRef = useRef(search);
  searchRef.current = search;

  const reportError = useCallback((err: unknown) => {
    const message =
      err instanceof AppError ? err.message : "发生了未预期的错误。";
    setNotice({ tone: "error", text: message });
  }, []);

  const reloadTracks = useCallback(
    async (query: string) => {
      try {
        setTracks(await api.listTracks(query));
      } catch (err) {
        reportError(err);
      }
    },
    [reportError],
  );

  const reloadRoots = useCallback(async () => {
    try {
      setRoots(await api.listRoots());
    } catch (err) {
      reportError(err);
    }
  }, [reportError]);

  // ── 启动时加载 ──────────────────────────────────────────
  useEffect(() => {
    void (async () => {
      await reloadRoots();
      await reloadTracks("");
      setBooted(true);
    })();
  }, [reloadRoots, reloadTracks]);

  // ── 搜索防抖 ────────────────────────────────────────────
  useEffect(() => {
    if (!booted) return;
    const timer = window.setTimeout(() => {
      void reloadTracks(search);
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [search, booted, reloadTracks]);

  /** 扫描一组根目录，返回是否全部成功。 */
  const scanRoots = useCallback(
    async (targets: LibraryRoot[]) => {
      if (targets.length === 0) return;

      setBusy(true);
      setNotice(null);
      let added = 0;
      let failed = 0;
      let skipped = 0;
      const allErrors: FileError[] = [];

      for (const root of targets) {
        setBusyLabel(`正在扫描 ${root.path}`);
        try {
          const summary = await api.scanRoot(root.id);
          added += summary.added;
          failed += summary.failed;
          skipped += summary.skipped;
          allErrors.push(...summary.errors);
        } catch (err) {
          const message =
            err instanceof AppError ? err.message : "扫描失败。";
          setNotice({ tone: "error", text: `${root.path}：${message}` });
          setBusy(false);
          setBusyLabel("");
          return;
        }
      }

      await reloadRoots();
      await reloadTracks(searchRef.current);
      setBusy(false);
      setBusyLabel("");

      const parts = [`新增 ${formatCount(added)} 首`];
      if (skipped > 0) parts.push(`跳过 ${formatCount(skipped)} 首（未变更）`);
      if (failed > 0) parts.push(`${failed} 个文件无法读取`);

      setNotice({
        tone: failed > 0 ? "warning" : "success",
        text: `扫描完成：${parts.join("，")}。`,
        errors: allErrors.length > 0 ? allErrors : undefined,
      });
    },
    [reloadRoots, reloadTracks],
  );

  // ── 用户操作 ────────────────────────────────────────────

  const handleAddRoot = useCallback(async () => {
    let picked: string | null = null;
    try {
      picked = await pickMusicFolder();
    } catch {
      setNotice({ tone: "error", text: "无法打开目录选择器。" });
      return;
    }
    if (!picked) return; // 用户取消

    setBusy(true);
    setBusyLabel("正在添加目录");
    try {
      const root = await api.addRoot(picked);
      setBusy(false);
      await scanRoots([root]);
    } catch (err) {
      setBusy(false);
      setBusyLabel("");
      reportError(err);
    }
  }, [reportError, scanRoots]);

  const handleScanAll = useCallback(() => {
    void scanRoots(roots.filter((r) => r.enabled));
  }, [roots, scanRoots]);

  const handleRescan = useCallback(
    (rootId: number) => {
      const root = roots.find((r) => r.id === rootId);
      if (root) void scanRoots([root]);
    },
    [roots, scanRoots],
  );

  const handleRemoveRoot = useCallback(
    async (rootId: number) => {
      const root = roots.find((r) => r.id === rootId);
      if (!root) return;

      // 移除只影响资料库索引，不动磁盘文件 —— 必须在文案里说清楚
      const confirmed = window.confirm(
        `从资料库中移除以下目录？\n\n${root.path}\n\n磁盘上的音乐文件不会被删除。`,
      );
      if (!confirmed) return;

      setBusy(true);
      setBusyLabel("正在移除目录");
      try {
        await api.removeRoot(rootId);
        await reloadRoots();
        await reloadTracks(searchRef.current);
        setNotice({ tone: "success", text: "目录已从资料库移除。" });
      } catch (err) {
        reportError(err);
      } finally {
        setBusy(false);
        setBusyLabel("");
      }
    },
    [roots, reloadRoots, reloadTracks, reportError],
  );

  // 视图切换回调必须保持稳定引用：它们是 NowPlaying 里 Esc 监听的依赖项，
  // 若每次渲染都新建，播放中（约 5 Hz 重渲染）监听器会被反复注销再注册。
  const openLibrary = useCallback(() => setView("library"), []);
  const openNowPlaying = useCallback(() => setView("nowPlaying"), []);

  // ── 渲染 ────────────────────────────────────────────────

  const trackCount = tracks.length;

  return (
    <div className="app">
      <Sidebar
        roots={roots}
        trackCount={trackCount}
        busy={busy}
        view={view}
        onOpenLibrary={openLibrary}
        onAddRoot={() => void handleAddRoot()}
        onRemoveRoot={(id) => void handleRemoveRoot(id)}
        onRescan={handleRescan}
      />

      <main className="main">
        <TopBar
          search={search}
          onSearchChange={setSearch}
          busy={busy}
          busyLabel={busyLabel}
          canScan={roots.length > 0}
          onScanAll={handleScanAll}
          canAdd={!busy}
          onAddRoot={() => void handleAddRoot()}
        />

        <div className="content">
          {notice && (
            <StatusBanner
              tone={notice.tone}
              errors={notice.errors}
              onDismiss={() => setNotice(null)}
            >
              {notice.text}
            </StatusBanner>
          )}

          {view === "nowPlaying" ? (
            <NowPlaying player={player} onBack={openLibrary} />
          ) : (
            <TrackListOrEmpty
              booted={booted}
              tracks={tracks}
              search={search}
              hasRoots={roots.length > 0}
              onAddRoot={() => void handleAddRoot()}
              playingTrackId={player.playingTrackId}
              isPlaying={player.snapshot?.status === "playing"}
              onPlay={(ids, start) => void player.playTracks(ids, start)}
            />
          )}
        </div>
      </main>

      <PlayerBar player={player} onOpenNowPlaying={openNowPlaying} />
    </div>
  );
}

function TrackListOrEmpty({
  booted,
  tracks,
  search,
  hasRoots,
  onAddRoot,
  playingTrackId,
  isPlaying,
  onPlay,
}: {
  booted: boolean;
  tracks: Track[];
  search: string;
  hasRoots: boolean;
  onAddRoot: () => void;
  playingTrackId: number | null;
  isPlaying: boolean;
  onPlay: (trackIds: number[], startIndex: number) => void;
}) {
  if (!booted) {
    return (
      <EmptyState icon="⏳" title="正在加载资料库…">
        <p>首次启动时会在此创建数据库。</p>
      </EmptyState>
    );
  }

  if (tracks.length === 0 && search.trim() !== "") {
    return (
      <EmptyState icon="⌕" title="没有匹配的曲目">
        <p>
          没有找到与「{search}」相关的曲目。试试其他关键词，或清空搜索框查看全部。
        </p>
      </EmptyState>
    );
  }

  if (tracks.length === 0 && !hasRoots) {
    return (
      <EmptyState
        icon="♪"
        title="开始建立你的音乐资料库"
        action={
          <button className="btn btn-primary" onClick={onAddRoot}>
            选择音乐目录
          </button>
        }
      >
        <p>
          选择一个包含音乐文件的目录，Aurora 会扫描其中的 MP3、FLAC、WAV、OGG
          与 M4A 文件并读取标题、艺术家、专辑等信息。
        </p>
        <p style={{ color: "var(--text-faint)", fontSize: "var(--text-sm)" }}>
          扫描过程只读取文件，不会修改、移动或删除你的任何音乐。
        </p>
      </EmptyState>
    );
  }

  if (tracks.length === 0) {
    return (
      <EmptyState icon="◌" title="资料库还是空的">
        <p>目录已添加，但还没有扫描到曲目。点击「扫描资料库」开始。</p>
      </EmptyState>
    );
  }

  return (
    <TrackList
      tracks={tracks}
      playingTrackId={playingTrackId}
      isPlaying={isPlaying}
      onPlay={onPlay}
    />
  );
}
