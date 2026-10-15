import type { LibraryRoot } from "../ipc/types";
import { formatCount } from "../lib/format";

interface Props {
  roots: LibraryRoot[];
  trackCount: number;
  busy: boolean;
  onAddRoot: () => void;
  onRemoveRoot: (rootId: number) => void;
  onRescan: (rootId: number) => void;
}

export function Sidebar({
  roots,
  trackCount,
  busy,
  onAddRoot,
  onRemoveRoot,
  onRescan,
}: Props) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark" aria-hidden="true" />
        <span className="brand-name">Aurora Music</span>
      </div>

      <nav className="nav" aria-label="主导航">
        <button className="nav-item" aria-current="true">
          <span className="nav-icon" aria-hidden="true">
            ♪
          </span>
          <span>音乐资料库</span>
          {trackCount > 0 && (
            <span className="nav-badge">{formatCount(trackCount)}</span>
          )}
        </button>

        {/* 后续里程碑实现，此处显式禁用而非隐藏，让用户知道功能存在 */}
        <button
          className="nav-item"
          aria-disabled="true"
          disabled
          title="将在后续版本实现"
        >
          <span className="nav-icon" aria-hidden="true">
            ♥
          </span>
          <span>收藏</span>
        </button>

        <button
          className="nav-item"
          aria-disabled="true"
          disabled
          title="将在后续版本实现"
        >
          <span className="nav-icon" aria-hidden="true">
            ↻
          </span>
          <span>最近播放</span>
        </button>
      </nav>

      <div className="section-heading">
        <span>音乐目录</span>
        <button
          className="root-remove"
          style={{ opacity: 1 }}
          onClick={onAddRoot}
          disabled={busy}
          title="添加音乐目录"
          aria-label="添加音乐目录"
        >
          ＋
        </button>
      </div>

      <div className="root-list">
        {roots.length === 0 ? (
          <p
            style={{
              color: "var(--text-faint)",
              fontSize: "var(--text-sm)",
              padding: "0 var(--space-3)",
              lineHeight: 1.6,
            }}
          >
            还没有添加目录。
            <br />
            点击上方 ＋ 选择音乐文件夹。
          </p>
        ) : (
          roots.map((root) => (
            <div key={root.id} className="root-item">
              <span className="nav-icon" aria-hidden="true">
                ◈
              </span>
              <span className="root-path" title={root.path}>
                {root.path}
              </span>
              <button
                className="root-remove"
                onClick={() => onRescan(root.id)}
                disabled={busy}
                title="重新扫描此目录"
                aria-label={`重新扫描 ${root.path}`}
              >
                ↻
              </button>
              <button
                className="root-remove"
                onClick={() => onRemoveRoot(root.id)}
                disabled={busy}
                title="从资料库移除（不会删除磁盘上的文件）"
                aria-label={`移除 ${root.path}`}
              >
                ✕
              </button>
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
