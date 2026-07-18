import type { ConnectionState, DaemonStatus } from '../lib/protocol';
import type { DiagnosticEvent } from '../lib/diagnostics';
import { isSupported, SUPPORTED_LOCALES } from '../l10n/locale';
import type { SupportedLocale } from '../l10n/locale';
import { useLocaleSettings } from '../l10n/strings';
import { LANGUAGE_NAMES, Punct } from '../l10n/tokens';
import { connStateInline, daemonMode } from '../l10n/wireDisplay';
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
  const {
    strings: s,
    textLocale,
    formattingLocale,
    setTextLocale,
    setFormattingLocale,
  } = useLocaleSettings();

  return (
    // Settings is a full-page destination, so it owns the page's `main`
    // landmark and its `h1` (issue #72). It is mounted on every route and
    // hidden by CSS elsewhere, which keeps it out of the accessibility tree —
    // so a second `main` is never exposed even though two are in the DOM.
    <main className="mobile-settings" id="settings-main" aria-labelledby="settings-title">
      <h1 className="mobile-settings-title" id="settings-title">
        {s.settingsTitle}
      </h1>
      <div className="settings-card settings-locale-card">
        <label className="field">
          <span>{s.settingsLanguageLabel}</span>
          <select
            value={textLocale ?? ''}
            onChange={(event) =>
              setTextLocale(event.target.value === '' ? null : (event.target.value as SupportedLocale))
            }
          >
            <option value="">{s.settingsLocaleSystemDefault}</option>
            {SUPPORTED_LOCALES.map((tag) => (
              <option key={tag} value={tag}>
                {LANGUAGE_NAMES[tag] ?? tag}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>{s.settingsFormattingLabel}</span>
          <select
            value={formattingLocale ?? ''}
            onChange={(event) => setFormattingLocale(event.target.value === '' ? null : event.target.value)}
          >
            <option value="">{s.settingsLocaleSystemDefault}</option>
            {formattingLocale !== null && !isSupported(formattingLocale) ? (
              // Preserve a valid custom/platform tag written by an earlier
              // client instead of making the picker falsely look like System.
              <option value={formattingLocale}>{formattingLocale}</option>
            ) : null}
            {SUPPORTED_LOCALES.map((tag) => (
              <option key={tag} value={tag}>
                {LANGUAGE_NAMES[tag] ?? tag}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="settings-card">
        <SelfLabelField value={selfLabel} onChange={onSetSelfLabel} />
      </div>
      <div className="settings-card">
        <span className="settings-label">{s.settingsIdentityLabel}</span>
        <code className="mono settings-val">{status?.identity?.identity_id ?? Punct.missingValue}</code>
      </div>
      <p className="muted settings-note">{s.settingsSelfLabelNote}</p>
      <div className="settings-card">
        <span className="settings-label">{s.settingsEndpointLabel}</span>
        <code className="mono settings-val">{status?.endpoint?.endpoint_id ?? Punct.missingValue}</code>
      </div>
      <div className="settings-card">
        <span className="settings-label">{s.settingsDaemonLabel}</span>
        <span className="settings-val">
          {status ? daemonMode(s, status.mode) : Punct.missingValue}
          {Punct.metaSep}
          {connStateInline(s, conn)}
        </span>
      </div>

      <div className="settings-card diagnostics-card">
        <div>
          <span className="settings-label">{s.settingsSupportLabel}</span>
          <h2 className="diagnostics-title">{s.settingsDiagnosticsTitle}</h2>
        </div>
        <p className="diagnostics-copy">{s.settingsDiagnosticsCopy}</p>
        <ul className="diagnostics-list">
          <li>{s.settingsNoMessageBodies}</li>
          <li>{s.settingsNoInviteTickets}</li>
          <li>{s.settingsNoFileNamesOrPaths}</li>
          <li>{s.settingsNoFullIdentityIds}</li>
        </ul>
        {lastDiagnosticError ? (
          <div className="diagnostics-last-error">
            <span className="settings-label">{s.settingsLastCapturedError}</span>
            <code>{lastDiagnosticError.context}</code>
            <span>{lastDiagnosticError.code}</span>
          </div>
        ) : (
          <p className="muted diagnostics-empty">{s.settingsNoErrorCaptured}</p>
        )}
        <div className="diagnostics-actions">
          <button type="button" className="btn btn-primary" onClick={onCopyDiagnostics}>
            {diagnosticsCopied ? s.settingsCopiedDiagnostics : s.settingsCopyDiagnostics}
          </button>
          <button type="button" className="btn btn-ghost" onClick={onReportIssue}>
            {s.settingsReportIssue}
          </button>
        </div>
      </div>

      <button type="button" className="btn btn-primary" onClick={onCreateRoom}>
        {s.roomsCreate}
      </button>
    </main>
  );
}
