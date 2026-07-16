import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import type { DaemonErrorShape, FileEntry, Member, PipeEntry, TimelineEvent } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { extOf, fileTint, formatBytes, formatTime, labelTone, prettyLabel } from '../lib/format';
import { useNames } from './names';
import { Avatar, ErrorNote, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchAvailability, FetchState } from './ui';

export type PanelTab = 'members' | 'agents' | 'files' | 'pipes';

export type PipeConnState =
  | { phase: 'connecting' }
  | { phase: 'connected'; local_addr: string }
  | { phase: 'error'; error: DaemonErrorShape };

// -- Members -------------------------------------------------------------------

function roleRank(role: Member['role']): number {
  if (role === 'owner') return 0;
  if (role === 'agent') return 1;
  return 2;
}

function statusRank(status: string): number {
  if (status === 'active') return 0;
  if (status === 'invited') return 1;
  if (status === 'left') return 2;
  if (status === 'removed') return 3;
  return 4;
}

function statusTone(status: string): 'active' | 'invited' | 'left' | 'removed' | 'unknown' {
  if (status === 'active' || status === 'invited' || status === 'left' || status === 'removed') return status;
  return 'unknown';
}

function displayStatus(status: string): string {
  return status ? status.charAt(0).toUpperCase() + status.slice(1) : 'Unknown';
}

function displayRole(role: Member['role']): string {
  return role === 'owner' ? 'Owner' : role === 'agent' ? 'Agent' : 'Member';
}

function shortMemberId(id: string): string {
  return id.length > 18 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id;
}

function MembersTab({
  members,
  selfId,
  onLeaveRoom,
}: {
  members: Member[];
  selfId: string | null;
  onLeaveRoom(): void;
}) {
  const names = useNames();
  const sorted = [...members].sort((a, b) => {
    const aSelf = selfId !== null && a.identity_id === selfId;
    const bSelf = selfId !== null && b.identity_id === selfId;
    if (aSelf !== bSelf) return aSelf ? -1 : 1;
    const byStatus = statusRank(a.status) - statusRank(b.status);
    if (byStatus !== 0) return byStatus;
    const byRole = roleRank(a.role) - roleRank(b.role);
    if (byRole !== 0) return byRole;
    return names.display(a.identity_id).localeCompare(names.display(b.identity_id));
  });
  const activeCount = members.filter((m) => m.status === 'active').length;
  const invitedCount = members.filter((m) => m.status === 'invited').length;
  const agentCount = members.filter((m) => m.role === 'agent').length;

  if (members.length === 0) {
    return <div className="panel-empty muted">No members have synced for this room yet.</div>;
  }

  return (
    <div className="panel-list members-panel">
      <section className="members-summary" aria-label="Room members summary">
        <div className="members-summary-copy">
          <h2>
            {members.length} room member{members.length === 1 ? '' : 's'}
          </h2>
          <p>Roster from the signed room history. Statuses reflect membership events, not live peer reachability.</p>
        </div>
        <dl className="member-stats" aria-label="Member counts">
          <div>
            <dt>Active</dt>
            <dd>{activeCount}</dd>
          </div>
          <div>
            <dt>Agents</dt>
            <dd>{agentCount}</dd>
          </div>
          {invitedCount > 0 ? (
            <div>
              <dt>Invited</dt>
              <dd>{invitedCount}</dd>
            </div>
          ) : null}
        </dl>
      </section>

      <div className="members-section-head">
        <h3>Room roster</h3>
        <span>{activeCount} active</span>
      </div>

      {sorted.map((member) => {
        const mine = selfId !== null && member.identity_id === selfId;
        const tone = statusTone(member.status);
        const canLeave = mine && member.status === 'active' && member.role !== 'owner';
        const ownerCannotLeave = mine && member.status === 'active' && member.role === 'owner';
        return (
          <div key={member.identity_id} className={`member-row member-row-${tone}`} title={member.identity_id}>
            <Avatar id={member.identity_id} size={38} />
            <div className="member-main">
              <div className="member-title-row">
                <SenderName id={member.identity_id} className="member-name" />
                {mine ? <span className="member-self-chip">this device</span> : null}
              </div>
              <code className="member-id mono">{shortMemberId(member.identity_id)}</code>
            </div>
            <div className="member-badges" aria-label={`${displayRole(member.role)}, ${displayStatus(member.status)}`}>
              <span className={`member-role role-${member.role}`}>{displayRole(member.role)}</span>
              <span className={`member-status status-${tone}`}>
                <span className="dot" /> {displayStatus(member.status)}
              </span>
            </div>
            {canLeave ? (
              <button type="button" className="btn btn-sm btn-danger member-leave-btn" onClick={onLeaveRoom}>
                Leave
              </button>
            ) : null}
            {ownerCannotLeave ? (
              <span className="member-owner-note" title="Owners cannot leave until ownership transfer exists.">
                Owner stays
              </span>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

// -- Agents --------------------------------------------------------------------

function AgentsTab({ members, timeline }: { members: Member[]; timeline: TimelineEvent[] }) {
  const agents = members.filter((m) => m.role === 'agent');
  if (agents.length === 0) {
    return <div className="panel-empty muted">No agent members in this room yet. Invite one with role “agent”.</div>;
  }
  return (
    <div className="panel-list">
      {agents.map((agent) => {
        let latest: TimelineEvent | undefined;
        for (let i = timeline.length - 1; i >= 0; i--) {
          const e = timeline[i];
          if (e.kind === 'agent_status' && e.sender.identity_id === agent.identity_id) {
            latest = e;
            break;
          }
        }
        return (
          <div key={agent.identity_id} className="agent-card">
            <div className="agent-head">
              <Avatar id={agent.identity_id} size={36} />
              <div className="agent-title">
                <SenderName id={agent.identity_id} className="agent-name" />
                {latest?.label ? (
                  <span className="agent-label">
                    <span className={`dot dot-${labelTone(latest.label)}`} /> {prettyLabel(latest.label)}
                  </span>
                ) : (
                  <span className="agent-label muted">No status posted yet</span>
                )}
              </div>
              {latest ? <time className="muted">{formatTime(latest.ts)}</time> : null}
            </div>
            {latest?.status_message ? <p className="agent-msg">{latest.status_message}</p> : null}
            {typeof latest?.progress === 'number' ? (
              <div className="progress-row">
                <ProgressBar value={latest.progress} />
                <span className="progress-num">{Math.max(0, Math.min(100, latest.progress))}%</span>
              </div>
            ) : null}
            <div className="agent-foot muted">status: {agent.status}</div>
          </div>
        );
      })}
    </div>
  );
}

// -- Files ---------------------------------------------------------------------

function mimeTypeLabel(mime: string, fallback: string): string {
  if (!mime) return fallback || 'file';
  const [type, subtype] = mime.split('/');
  if (!subtype) return mime;
  if (subtype === 'octet-stream') return fallback || 'binary';
  if (type === 'text' && subtype === 'plain') return 'text';
  return subtype.replace(/[.+-]/g, ' ');
}

function fileTypeLabel(file: FileEntry, ext: string): string {
  return mimeTypeLabel(file.mime, ext || 'file');
}

function selectedFileTypeLabel(file: File): string {
  const ext = extOf(file.name).toLowerCase();
  return mimeTypeLabel(file.type, ext || 'file');
}

function FilesTab({
  files,
  selfId,
  fetches,
  onFetch,
  onRecheckFiles,
  onSharePath,
  onShareBrowserFile,
}: {
  files: FileEntry[];
  selfId: string | null;
  fetches: Record<string, FetchState>;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onSharePath(path: string): Promise<void>;
  onShareBrowserFile(file: File): Promise<void>;
}) {
  const [path, setPath] = useState('');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [pickerKey, setPickerKey] = useState(0);
  const [sharing, setSharing] = useState(false);
  const [shareError, setShareError] = useState<DaemonErrorShape | null>(null);
  const totalBytes = files.reduce((sum, file) => sum + file.size, 0);
  const availableCount = files.filter((file) => file.available).length;
  const fetchedCount = files.filter((file) => {
    const state = fetches[file.file_id];
    return state?.phase === 'verified' || state?.phase === 'fetched' || file.fetched;
  }).length;
  const providerCount = files.reduce((sum, file) => sum + file.providers, 0);
  // A file this device shared reports available:false from its own view (the
  // daemon excludes self from the provider set), so it is neither fetchable here
  // nor a fault. Count those separately to keep the summary honest.
  const servingCount = files.filter((file) => !file.available && selfId !== null && file.sender_id === selfId).length;
  const waitingProviderCount = files.filter((file) => {
    const state = fetches[file.file_id];
    const fetched = state?.phase === 'verified' || state?.phase === 'fetched' || file.fetched;
    const serving = !file.available && selfId !== null && file.sender_id === selfId;
    return !file.available && !serving && !fetched;
  }).length;
  const heroDetail =
    files.length === 0
      ? 'Share a readable path and peers can fetch a verified copy over P2P.'
      : `${formatBytes(totalBytes)} in the room · ${availableCount} fetchable here` +
        (fetchedCount > 0 ? ` · ${fetchedCount} fetched` : '') +
        (servingCount > 0 ? ` · ${servingCount} served by you` : '');
  const shareHelpId = 'file-share-help';
  const selectedType = selectedFile ? selectedFileTypeLabel(selectedFile) : null;

  const share = async () => {
    const p = path.trim();
    if (sharing || (!selectedFile && !p)) return;
    setSharing(true);
    setShareError(null);
    try {
      if (selectedFile) {
        await onShareBrowserFile(selectedFile);
        setSelectedFile(null);
        setPickerKey((key) => key + 1);
      } else {
        await onSharePath(p);
        setPath('');
      }
    } catch (e) {
      setShareError(errorShape(e));
    } finally {
      setSharing(false);
    }
  };

  return (
    <div className="panel-list files-panel">
      <section className={`files-hero${files.length === 0 ? ' is-empty' : ''}`} aria-label="Files summary">
        <span className="files-hero-mark" aria-hidden="true">
          ▤
        </span>
        <div className="files-hero-copy">
          <h2>{files.length === 0 ? 'No shared files yet' : `${files.length} shared file${files.length === 1 ? '' : 's'}`}</h2>
          <p>{heroDetail}</p>
        </div>
        {files.length > 0 ? (
          <dl className="files-stats" aria-label="File availability">
            <div>
              <dt>Fetchable now</dt>
              <dd>
                {availableCount}/{files.length}
              </dd>
            </div>
            <div>
              <dt>Provider devices</dt>
              <dd>{providerCount}</dd>
            </div>
          </dl>
        ) : null}
      </section>

      <form
        className="panel-form file-share-card"
        onSubmit={(e) => {
          e.preventDefault();
          void share();
        }}
      >
        <div className="panel-form-head">
          <div>
            <h2>Choose a file to share</h2>
            <p className="muted" id={shareHelpId}>
              Pick a local file. Jeliya uploads it to this daemon, imports it into the room blob store, and verifies it by content hash.
            </p>
          </div>
          <span className="file-share-badge" aria-label="Verified by content hash">
            hash checked
          </span>
        </div>

        <div className="file-picker-shell">
          <input
            key={pickerKey}
            className="file-picker-input"
            type="file"
            onChange={(e) => {
              const file = e.currentTarget.files?.[0] ?? null;
              setSelectedFile(file);
              if (file) setPath('');
            }}
            aria-label="Choose file to share"
            aria-describedby={shareHelpId}
          />
          {selectedFile ? (
            <div className="selected-file-card" aria-live="polite">
              <span className="selected-file-icon" aria-hidden="true">
                {extOf(selectedFile.name).toUpperCase().slice(0, 4) || 'FILE'}
              </span>
              <div className="selected-file-info">
                <strong>{selectedFile.name}</strong>
                <span className="muted">
                  {formatBytes(selectedFile.size)} · {selectedType}
                </span>
              </div>
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => {
                  setSelectedFile(null);
                  setPickerKey((key) => key + 1);
                }}
              >
                Clear
              </button>
            </div>
          ) : (
            <p className="file-picker-empty muted">No file selected yet.</p>
          )}
        </div>

        <button type="submit" className="btn btn-primary file-share-submit" disabled={sharing || (!selectedFile && !path.trim())}>
          {sharing ? 'Sharing…' : 'Share'}
        </button>

        <details className="file-advanced-path">
          <summary>Advanced: paste a daemon-readable path</summary>
          <div className="form-row file-share-row">
            <input
              value={path}
              onChange={(e) => {
                setPath(e.target.value);
                if (e.target.value.trim()) {
                  setSelectedFile(null);
                  setPickerKey((key) => key + 1);
                }
              }}
              placeholder="/path/to/report.pdf"
              aria-label="File path to share"
              spellCheck={false}
            />
          </div>
          <p className="form-hint muted">Use this only for files already under the daemon data directory.</p>
        </details>
        <ErrorNote error={shareError} />
      </form>

      {files.length > 0 ? (
        <div className="files-section-head">
          <h3>Shared in this room</h3>
          <span>
            {availableCount === files.length
              ? 'All fetchable'
              : [
                  servingCount > 0 ? `${servingCount} served by you` : null,
                  waitingProviderCount > 0 ? `${waitingProviderCount} awaiting a provider` : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
          </span>
        </div>
      ) : null}

      {files.map((file) => {
        const tint = fileTint(file.name);
        const ext = extOf(file.name).toUpperCase() || 'FILE';
        const type = fileTypeLabel(file, ext.toLowerCase());
        const mine = selfId !== null && file.sender_id === selfId;
        const fetchState = fetches[file.file_id];
        const availability: FetchAvailability = { available: file.available, providers: file.providers };
        const fetched = fetchState?.phase === 'verified' || fetchState?.phase === 'fetched' || file.fetched;
        const failedState = fetchState?.phase === 'error' ? fetchState : null;
        // `available` is "another provider device is a connected peer right now"
        // (the daemon excludes THIS device), so a file you shared reads as
        // not-available from your own view even though peers can fetch it. Label
        // by ownership so that is never shown as a fault. See list_files() in
        // crates/jeliya-core/src/supervisor.rs.
        const health = mine
          ? { tone: 'self', text: 'Serving to peers' }
          : fetched
            ? { tone: 'ok', text: 'Fetched locally' }
          : failedState
            ? {
                tone: 'warn',
                text: failedState.error.code === 'hash_mismatch' ? 'Security check failed' : 'Fetch failed',
              }
          : file.available
            ? { tone: 'ok', text: 'Ready to fetch' }
            : { tone: 'warn', text: 'No provider online' };
        const providerText = `${file.providers} provider${file.providers === 1 ? '' : 's'}`;
        return (
          <div
            key={file.file_id}
            className={`file-row${!file.available && !mine && !fetched ? ' unavailable' : ''}`}
            title={file.file_id}
          >
            <div className="file-row-main">
              <span className="file-icon" style={{ background: `${tint}22`, color: tint }} aria-hidden="true">
                {ext.slice(0, 4)}
              </span>
              <div className="file-row-info">
                <div className="file-title-row">
                  <strong>{file.name}</strong>
                  <span className="file-kind">{type}</span>
                </div>
                <span className="file-meta muted">
                  {formatBytes(file.size)} · <SenderName id={file.sender_id} /> · {formatTime(file.ts)}
                </span>
                <span className={`file-health ${health.tone}`}>
                  <span className="dot" />
                  {health.text} · {providerText}
                </span>
              </div>
              <div className="file-row-action">
                {mine ? (
                  <span className="file-self-note" title="This daemon is already serving this file to peers.">
                    Serving
                  </span>
                ) : (
                  <FetchControl
                    state={fetchState}
                    availability={availability}
                    onFetch={() => onFetch(file.file_id)}
                    onRecheck={onRecheckFiles}
                  />
                )}
              </div>
            </div>
            {mine ? null : <FetchDetail state={fetchState} />}
          </div>
        );
      })}
    </div>
  );
}

// -- Pipes ---------------------------------------------------------------------

function PipesTab({
  pipes,
  members,
  selfId,
  conns,
  closing,
  focusPipeId = null,
  onFocusPipeHandled,
  onConnect,
  onClose,
  onExpose,
}: {
  pipes: PipeEntry[];
  members: Member[];
  selfId: string | null;
  conns: Record<string, PipeConnState>;
  closing: Set<string>;
  focusPipeId?: string | null;
  onFocusPipeHandled?(): void;
  onConnect(pipeId: string): void;
  onClose(pipeId: string): void;
  onExpose(target: string, peerIdentity: string): Promise<void>;
}) {
  const names = useNames();
  const [target, setTarget] = useState('');
  const peerChoices = members.filter((m) => m.identity_id !== selfId);
  const [peer, setPeer] = useState('');
  const [exposing, setExposing] = useState(false);
  const [exposeError, setExposeError] = useState<DaemonErrorShape | null>(null);

  // "Open in Pipes" lands here with the pipe it came from: move focus to that
  // row and mark it so the destination identifies the relevant item instead
  // of appearing unchanged (the row may be far down a long list).
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const [flashPipeId, setFlashPipeId] = useState<string | null>(null);
  useEffect(() => {
    if (!focusPipeId) return;
    const row = rowRefs.current.get(focusPipeId);
    if (row) {
      row.scrollIntoView({ block: 'nearest' });
      row.focus({ preventScroll: true });
      setFlashPipeId(focusPipeId);
    }
    // Consume the request either way (an unknown id must not re-fire forever).
    onFocusPipeHandled?.();
  }, [focusPipeId, onFocusPipeHandled]);
  useEffect(() => {
    if (flashPipeId === null) return;
    const timer = window.setTimeout(() => setFlashPipeId(null), 1600);
    return () => window.clearTimeout(timer);
  }, [flashPipeId]);

  const expose = async () => {
    const t = target.trim();
    const p = peer || peerChoices[0]?.identity_id || '';
    if (!t || !p || exposing) return;
    setExposing(true);
    setExposeError(null);
    try {
      await onExpose(t, p);
      setTarget('');
    } catch (e) {
      setExposeError(errorShape(e));
    } finally {
      setExposing(false);
    }
  };

  return (
    <div className="panel-list">
      {pipes.length === 0 ? (
        <div className="panel-empty muted">No pipes yet — expose a local port to one authorized peer below.</div>
      ) : null}
      {pipes.map((pipe) => {
        const conn = conns[pipe.pipe_id];
        return (
          <div
            key={pipe.pipe_id}
            ref={(el) => {
              if (el) rowRefs.current.set(pipe.pipe_id, el);
              else rowRefs.current.delete(pipe.pipe_id);
            }}
            tabIndex={-1}
            className={`pipe-row${pipe.state === 'closed' ? ' closed' : ''}${
              flashPipeId === pipe.pipe_id ? ' pipe-row-flash' : ''
            }`}
          >
            <div className="pipe-row-head">
              <span className="pipe-icon" aria-hidden="true">
                ⤳
              </span>
              <strong className="mono">{pipe.target}</strong>
              <span className={`chip chip-state state-${pipe.state}`}>
                {pipe.state === 'open' ? (pipe.connected ? 'Active' : 'Open') : 'Closed'}
              </span>
            </div>
            <div className="pipe-row-meta muted">
              by <SenderName id={pipe.opened_by} /> · authorized:{' '}
              {pipe.authorized_peer
                ? names.isSelf(pipe.authorized_peer)
                  ? 'You'
                  : <SenderName id={pipe.authorized_peer} />
                : '—'}
            </div>
            {pipe.state === 'open' ? (
              <div className="pipe-row-actions">
                {!conn || conn.phase === 'error' ? (
                  <button type="button" className="btn btn-sm" onClick={() => onConnect(pipe.pipe_id)}>
                    Connect
                  </button>
                ) : conn.phase === 'connecting' ? (
                  <button type="button" className="btn btn-sm" disabled>
                    <span className="spinner" aria-hidden="true" /> Connecting…
                  </button>
                ) : (
                  <>
                    <code className="pipe-addr mono">{conn.local_addr}</code>
                    <a
                      className="btn btn-sm btn-primary"
                      href={`http://${conn.local_addr}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      Open preview ↗
                    </a>
                  </>
                )}
                {closing.has(pipe.pipe_id) ? (
                  <button type="button" className="btn btn-sm btn-ghost" disabled>
                    <span className="spinner" aria-hidden="true" /> Closing…
                  </button>
                ) : (
                  <button type="button" className="btn btn-sm btn-ghost" onClick={() => onClose(pipe.pipe_id)}>
                    Close
                  </button>
                )}
              </div>
            ) : null}
            {conn?.phase === 'error' ? <ErrorNote error={conn.error} /> : null}
          </div>
        );
      })}

      <form
        className="panel-form"
        onSubmit={(e) => {
          e.preventDefault();
          void expose();
        }}
      >
        <h2>Expose a pipe</h2>
        <p className="muted">Forward a local port to exactly one authorized peer.</p>
        <div className="form-row">
          <input
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="127.0.0.1:3000"
            aria-label="Local target (host:port)"
            spellCheck={false}
          />
        </div>
        <div className="form-row">
          <select
            value={peer || peerChoices[0]?.identity_id || ''}
            onChange={(e) => setPeer(e.target.value)}
            aria-label="Authorized peer"
          >
            {peerChoices.length === 0 ? <option value="">no other members</option> : null}
            {peerChoices.map((m) => (
              <option key={m.identity_id} value={m.identity_id}>
                {names.display(m.identity_id)} ({m.role})
              </option>
            ))}
          </select>
          <button type="submit" className="btn" disabled={exposing || !target.trim() || peerChoices.length === 0}>
            {exposing ? 'Exposing…' : 'Expose'}
          </button>
        </div>
        <ErrorNote error={exposeError} />
      </form>
    </div>
  );
}

// -- panel shell -----------------------------------------------------------------

function tabCountLabel(count: number): string {
  return count > 99 ? '99+' : String(count);
}

export function RightPanel({
  tab,
  onTab,
  roomName,
  members,
  timeline,
  files,
  pipes,
  selfId,
  fetches,
  focusPipeId = null,
  onFocusPipeHandled,
  onFetch,
  onRecheckFiles,
  onSharePath,
  onShareBrowserFile,
  pipeConns,
  closingPipes,
  onPipeConnect,
  onPipeClose,
  onPipeExpose,
  onLeaveRoom,
}: {
  tab: PanelTab;
  onTab(tab: PanelTab): void;
  roomName?: string | null;
  members: Member[];
  timeline: TimelineEvent[];
  files: FileEntry[];
  pipes: PipeEntry[];
  selfId: string | null;
  fetches: Record<string, FetchState>;
  focusPipeId?: string | null;
  onFocusPipeHandled?(): void;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onSharePath(path: string): Promise<void>;
  onShareBrowserFile(file: File): Promise<void>;
  pipeConns: Record<string, PipeConnState>;
  closingPipes: Set<string>;
  onPipeConnect(pipeId: string): void;
  onPipeClose(pipeId: string): void;
  onPipeExpose(target: string, peerIdentity: string): Promise<void>;
  onLeaveRoom(): void;
}) {
  const agentCount = members.filter((m) => m.role === 'agent').length;
  const openPipes = pipes.filter((p) => p.state === 'open').length;

  const tabs: { id: PanelTab; label: string; count: number }[] = [
    { id: 'members', label: 'Members', count: members.length },
    { id: 'agents', label: 'Agents', count: agentCount },
    { id: 'files', label: 'Files', count: files.length },
    { id: 'pipes', label: 'Pipes', count: openPipes },
  ];

  // Full ARIA tabs keyboard pattern: arrow keys move between tabs (a single tab
  // stop via roving tabindex), Home/End jump to the ends. Without this the tab
  // roles would announce a pattern that does not actually work.
  const onTabsKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const idx = tabs.findIndex((t) => t.id === tab);
    let next = idx;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (idx + 1) % tabs.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    else return;
    e.preventDefault();
    onTab(tabs[next].id);
    document.getElementById(`panel-tab-${tabs[next].id}`)?.focus();
  };

  return (
    <aside className="right-panel">
      {/* Mobile-only: on the standalone Files/Pipes tab this panel is the whole
          screen and RoomHeader (which normally carries the room name) is off in
          the hidden `.center` pane, so without this a multi-room user has no way
          to tell which room they're looking at. Not aria-hidden — this is the
          only place the room name reaches the accessible tree in that view. */}
      {roomName ? <div className="panel-room-context">{roomName}</div> : null}
      <div className="panel-tabs" role="tablist" aria-label="Room panel" onKeyDown={onTabsKeyDown}>
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={`panel-tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls="panel-body"
            tabIndex={tab === t.id ? 0 : -1}
            className={tab === t.id ? 'active' : ''}
            onClick={() => onTab(t.id)}
          >
            <span className="panel-tab-label">{t.label}</span>
            {t.count > 0 ? <span className="count">{tabCountLabel(t.count)}</span> : null}
          </button>
        ))}
      </div>
      <div className="panel-body" id="panel-body" role="tabpanel" aria-labelledby={`panel-tab-${tab}`}>
        {tab === 'members' ? <MembersTab members={members} selfId={selfId} onLeaveRoom={onLeaveRoom} /> : null}
        {tab === 'agents' ? <AgentsTab members={members} timeline={timeline} /> : null}
        {tab === 'files' ? (
          <FilesTab
            files={files}
            selfId={selfId}
            fetches={fetches}
            onFetch={onFetch}
            onRecheckFiles={onRecheckFiles}
            onSharePath={onSharePath}
            onShareBrowserFile={onShareBrowserFile}
          />
        ) : null}
        {tab === 'pipes' ? (
          <PipesTab
            pipes={pipes}
            members={members}
            selfId={selfId}
            conns={pipeConns}
            closing={closingPipes}
            focusPipeId={focusPipeId}
            onFocusPipeHandled={onFocusPipeHandled}
            onConnect={onPipeConnect}
            onClose={onPipeClose}
            onExpose={onPipeExpose}
          />
        ) : null}
      </div>
    </aside>
  );
}
