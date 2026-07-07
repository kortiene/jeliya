import type { ConnectionState, DaemonStatus } from '../lib/protocol';
import type { DiagnosticEvent } from '../lib/diagnostics';

export function SettingsPanel({
  status,
  conn,
  diagnosticsCopied,
  lastDiagnosticError,
  onCopyDiagnostics,
  onReportIssue,
  onCreateRoom,
}: {
  status: DaemonStatus | null;
  conn: ConnectionState;
  diagnosticsCopied: boolean;
  lastDiagnosticError: DiagnosticEvent | null;
  onCopyDiagnostics(): void;
  onReportIssue(): void;
  onCreateRoom(): void;
}) {
  return (
    <section className="mobile-settings" aria-label="Settings">
      <h2 className="mobile-settings-title">Settings</h2>
      <div className="settings-card">
        <span className="settings-label">P2P Identity</span>
        <code className="mono settings-val">{status?.identity?.identity_id ?? '-'}</code>
      </div>
      <p className="muted settings-note">Unrecoverable if this device or its data folder is lost.</p>
      <div className="settings-card">
        <span className="settings-label">Endpoint</span>
        <code className="mono settings-val">{status?.endpoint?.endpoint_id ?? '-'}</code>
      </div>
      <div className="settings-card">
        <span className="settings-label">Daemon</span>
        <span className="settings-val">
          {status?.mode ?? '-'} · {conn}
        </span>
      </div>

      <div className="settings-card diagnostics-card">
        <div>
          <span className="settings-label">Support</span>
          <h3 className="diagnostics-title">Diagnostics</h3>
        </div>
        <p className="diagnostics-copy">
          Copy a privacy-safe snapshot for bug reports: daemon version, connection state, room counts, peer state,
          file-transfer state, pipe state, and the latest UI error.
        </p>
        <ul className="diagnostics-list">
          <li>No message bodies</li>
          <li>No invite tickets</li>
          <li>No file names or full local paths</li>
          <li>No full identity IDs</li>
        </ul>
        {lastDiagnosticError ? (
          <div className="diagnostics-last-error">
            <span className="settings-label">Last captured error</span>
            <code>{lastDiagnosticError.context}</code>
            <span>{lastDiagnosticError.code}</span>
          </div>
        ) : (
          <p className="muted diagnostics-empty">No UI action error captured in this session.</p>
        )}
        <div className="diagnostics-actions">
          <button type="button" className="btn btn-primary" onClick={onCopyDiagnostics}>
            {diagnosticsCopied ? 'Copied diagnostics' : 'Copy diagnostics'}
          </button>
          <button type="button" className="btn btn-ghost" onClick={onReportIssue}>
            Report issue
          </button>
        </div>
      </div>

      <button type="button" className="btn btn-primary" onClick={onCreateRoom}>
        Create a room
      </button>
    </section>
  );
}
