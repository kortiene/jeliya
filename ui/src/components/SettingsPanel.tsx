import type { ConnectionState, DaemonStatus } from '../lib/protocol';
import type { DiagnosticEvent } from '../lib/diagnostics';
import { SelfLabelField } from './ui';

export function SettingsPanel({
  status,
  conn,
  selfLabel,
  onSetSelfLabel,
  diagnosticsCopied,
  lastDiagnosticError,
  onCopyDiagnostics,
  onReportIssue,
  onCreateRoom,
}: {
  status: DaemonStatus | null;
  conn: ConnectionState;
  selfLabel: string;
  onSetSelfLabel(label: string): void;
  diagnosticsCopied: boolean;
  lastDiagnosticError: DiagnosticEvent | null;
  onCopyDiagnostics(): void;
  onReportIssue(): void;
  onCreateRoom(): void;
}) {
  return (
    // Settings is a full-page destination, so it owns the page's `main`
    // landmark and its `h1` (issue #72). It is mounted on every route and
    // hidden by CSS elsewhere, which keeps it out of the accessibility tree —
    // so a second `main` is never exposed even though two are in the DOM.
    <main className="mobile-settings" id="settings-main" aria-labelledby="settings-title">
      <h1 className="mobile-settings-title" id="settings-title">
        Settings
      </h1>
      <div className="settings-card">
        <SelfLabelField value={selfLabel} onChange={onSetSelfLabel} />
      </div>
      <div className="settings-card">
        <span className="settings-label">P2P Identity</span>
        <code className="mono settings-val">{status?.identity?.identity_id ?? '-'}</code>
      </div>
      <p className="muted settings-note">
        Your name is a local label — it never changes your cryptographic identity, which is unrecoverable if this
        device or its data folder is lost.
      </p>
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
          <h2 className="diagnostics-title">Diagnostics</h2>
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
    </main>
  );
}
