interface Props {
  search: string;
  onSearchChange: (value: string) => void;
  busy: boolean;
  busyLabel: string;
  canScan: boolean;
  onScanAll: () => void;
  canAdd: boolean;
  onAddRoot: () => void;
}

export function TopBar({
  search,
  onSearchChange,
  busy,
  busyLabel,
  canScan,
  onScanAll,
  canAdd,
  onAddRoot,
}: Props) {
  return (
    <header className="topbar">
      <div className="search">
        <span className="search-icon" aria-hidden="true">
          ⌕
        </span>
        <input
          className="search-input"
          type="search"
          value={search}
          onChange={(e) => onSearchChange(e.currentTarget.value)}
          placeholder="搜索曲目标题、艺术家或专辑…"
          aria-label="搜索曲目"
          spellCheck={false}
        />
        {search && (
          <button
            className="search-clear"
            onClick={() => onSearchChange("")}
            aria-label="清除搜索"
          >
            ✕
          </button>
        )}
      </div>

      <span className="spacer" />

      {busy && (
        <span className="busy" role="status">
          <span className="spinner" aria-hidden="true" />
          {busyLabel}
        </span>
      )}

      <button
        className="btn btn-ghost"
        onClick={onScanAll}
        disabled={busy || !canScan}
        title={canScan ? "重新扫描全部目录" : "请先添加音乐目录"}
      >
        扫描资料库
      </button>

      <button
        className="btn btn-primary"
        onClick={onAddRoot}
        disabled={busy || !canAdd}
      >
        添加目录
      </button>
    </header>
  );
}
