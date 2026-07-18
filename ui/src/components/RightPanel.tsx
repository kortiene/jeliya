import { useEffect, useRef, useState } from 'react';
import type { DaemonErrorShape, FileEntry, Member, PipeEntry, TimelineEvent } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { extOf, fileTint, formatBytes, formatTime, labelTone, prettyLabel } from '../lib/format';
import type { InspectorDest, RoomDest } from '../lib/routes';
import type { Shell } from '../lib/shell';
import { RoomNav } from './RoomNav';
import { useNames } from './names';
import { Avatar, ErrorNote, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchAvailability, FetchState } from './ui';

/** The inspector renders one room tool, and which one is the route's job
 *  (docs/room-workbench.md, decision 3) — so the tab strip is a view of
 *  `InspectorDest`, never a second place navigation state can live. */
export type PanelTab = InspectorDest;

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

/** Signed roster status, as a label (docs/room-workbench.md, decision 4).
 *
 *  This used to title-case the wire value, which rendered the roster's
 *  `active` as "Active" — the same word the room rail used for a live local
 *  session, and the collision the record exists to remove. Display labels and
 *  wire values are never the same constant (docs/i18n.md, rule 3). */
function displayStatus(status: string): string {
  switch (status) {
    case 'active':
      return 'Member';
    case 'invited':
      return 'Invited';
    case 'left':
      return 'Left';
    case 'removed':
      return 'Removed';
    default:
      return 'Unknown';
  }
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
            {members.length} in the roster
          </h2>
          <p>Roster from the signed room history. Statuses reflect membership events, not live peer reachability.</p>
        </div>
        <dl className="member-stats" aria-label="Member counts">
          <div>
            {/* The roster holds everyone with a membership event; only some of
                them are currently members. Calling the whole list "members"
                and this count "active" made the two disagree by construction. */}
            <dt>Members</dt>
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
        <span>
          {activeCount} member{activeCount === 1 ? '' : 's'}
        </span>
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
      onFocusPipeHandled?.();
    } else if (pipes.length > 0 && !pipes.some((p) => p.pipe_id === focusPipeId)) {
      // The list is in and the pipe genuinely isn't there (closed/unknown):
      // consume so a dead id can't re-fire forever.
      onFocusPipeHandled?.();
    }
    // Otherwise pipe.list is still loading — keep the request pending; the
    // effect re-runs when `pipes` arrives. Consuming here would silently
    // drop the identification the user just asked for.
  }, [focusPipeId, pipes, onFocusPipeHandled]);
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
              {/* Pipe connection, and only that (decision 4): exposed with a
                  live forwarding session, exposed with none, or closed. */}
              <span className={`chip chip-state state-${pipe.state}`}>
                {pipe.state === 'open' ? (pipe.connected ? 'Connected' : 'Open') : 'Closed'}
              </span>
            </div>
            <div className="pipe-row-meta muted">
              by <SenderName id={pipe.opened_by} /> · authorized:{' '}
              {pipe.authorized_peer ? <SenderName id={pipe.authorized_peer} /> : '—'}
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

export function RightPanel({
  tab,
  onDest,
  counts,
  onClose,
  shell,
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
  /** Compact renders the room nav inside this panel, so it needs the same
   *  inputs the workspace strip gets. */
  onDest(dest: RoomDest): void;
  counts: Partial<Record<RoomDest, number>>;
  /** Close the inspector — which is navigating to the room's Activity
   *  (docs/room-workbench.md, decision 3), not a local visibility toggle. */
  onClose(): void;
  /** Medium floats this over the workspace as a dismissible drawer; wide
   *  places it in flow as a column; compact gives it the whole pane. */
  shell: Shell;
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
  return (
    <aside className={`right-panel${shell === 'medium' ? ' right-panel-drawer' : ''}`}>
      {/* The room stays named on every room-scoped surface (decision 3). On
          compact this panel IS the screen and the room header is off in the
          hidden `.center` pane; on medium it floats over the workspace. Not
          aria-hidden — on compact this is the only place the room name reaches
          the accessible tree. */}
      <div className="panel-head">
        {/* One navigation, three affordances. Compact needs a Back of its own:
            this panel is the whole screen there and the bottom bar is gone, so
            without it the only way out would be the browser's own Back — which
            an installed PWA does not show. */}
        {shell === 'compact' ? (
          <button type="button" className="icon-btn panel-back" onClick={onClose} aria-label="Back to Activity">
            <span aria-hidden="true">‹</span>
          </button>
        ) : null}
        {roomName ? <div className="panel-room-context">{roomName}</div> : <span />}
        {shell !== 'compact' ? (
          <button type="button" className="icon-btn panel-close" onClick={onClose} aria-label="Close inspector">
            <span aria-hidden="true">×</span>
          </button>
        ) : null}
      </div>
      {/* Whenever this panel covers the workspace — the whole pane on compact,
          a drawer on medium — it owes the room's nav, because the workspace's
          copy is underneath it. Only on wide, where the panel opens beside the
          workspace rather than over it, does the workspace keep the strip and
          this render nothing: two tablists for one room would be two tablists
          in the a11y tree. */}
      {shell !== 'wide' ? <RoomNav dest={tab} counts={counts} onDest={onDest} /> : null}
      <div className="panel-body" id="panel-body" role="tabpanel" aria-labelledby={`room-tab-${tab}`}>
        {tab === 'people' ? <MembersTab members={members} selfId={selfId} onLeaveRoom={onLeaveRoom} /> : null}
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
