import type { FileError } from "../ipc/types";

export function EmptyState({
  icon,
  title,
  children,
  action,
}: {
  icon: string;
  title: string;
  children?: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-mark" aria-hidden="true">
        {icon}
      </div>
      <h2 className="empty-title">{title}</h2>
      {children && <div className="empty-body">{children}</div>}
      {action}
    </div>
  );
}

type Tone = "error" | "warning" | "success";

export function StatusBanner({
  tone,
  children,
  errors,
  onDismiss,
}: {
  tone: Tone;
  children: React.ReactNode;
  errors?: FileError[];
  onDismiss?: () => void;
}) {
  return (
    <div className={`status-banner ${tone}`} role={tone === "error" ? "alert" : "status"}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div>{children}</div>
        {errors && errors.length > 0 && (
          <ul className="status-detail">
            {errors.map((e, i) => (
              <li key={`${e.name}-${i}`}>
                {e.name} — {e.reason}
              </li>
            ))}
          </ul>
        )}
      </div>
      {onDismiss && (
        <button className="btn btn-ghost" onClick={onDismiss} aria-label="关闭提示">
          ✕
        </button>
      )}
    </div>
  );
}
