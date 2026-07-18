import { useEffect, useRef, useState } from 'react';
import type { DaemonErrorShape, FileEntry, Member, PipeEntry, TimelineEvent } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { extOf, fileTint, labelTone, prettyLabel } from '../lib/format';
import type { InspectorDest, RoomDest } from '../lib/routes';
import type { Shell } from '../lib/shell';
import type { Catalog } from '../l10n/catalog';
import { roomDestLabel } from '../l10n/destinations';
import { scrollIntoView } from '../lib/motion';
import { useFormats, useStrings } from '../l10n/strings';
import { Template } from '../l10n/template';
import { Example, Glyph, Punct } from '../l10n/tokens';
import { memberStatus, roleInline, rolePill } from '../l10n/wireDisplay';
import { RoomNav } from './RoomNav';
import { useNames } from './names';
import { Avatar, ErrorNote, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchAvailability, FetchState } from './ui';

/** The inspector renders one room tool, and which one is the route's job
 *  (docs/room-workbench.md, decision 3) — so the tab strip is a view of
 *  `InspectorDest`, never a second place navigation state can live. */
export type PanelTab = InspectorDest;

export type PipeConnState =
  { phase: 'connecting' } | { phase: 'connected'; local_addr: string } | { phase: 'error'; error: DaemonErrorShape };

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
  const s = useStrings();
  const formats = useFormats();
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
    return <div className="panel-empty muted">{s.panelMembersEmpty}</div>;
  }

  return (
    <div className="panel-list members-panel">
      {/* A plain section, not a labelled one: an `aria-label` here would make
          this a `region` landmark nested inside the inspector's own landmark,
          which is noise — the h2 below already structures it (issue #72). */}
      <section className="members-summary">
        <div className="members-summary-copy">
          <h2>{s.panelRoomMemberCount(members.length, formats.count(members.length))}</h2>
          <p>{s.panelRosterCopy}</p>
        </div>
      </section>

      {/* The breakdown of the roster, stated once. It used to be a
          `.member-stats` grid of hero-metric tiles here AND a "{activeCount}
          member(s)" line eighteen rows down — the same number twice inside one
          scroll view, in the very tile pattern PRODUCT.md names as an
          anti-reference. The tiles are gone and the counts moved onto the
          section head that was already carrying one of them, so each fact
          renders exactly once, next to the list it describes.

          The roster holds everyone who has a membership event; only some of
          them are currently members — so `members.length` above and this
          `activeCount` are genuinely different numbers, and both keep a name
          that says which one it is. */}
      <div className="members-section-head">
        <h3>{s.panelRoomRoster}</h3>
        <span>
          {[
            s.commonMemberCount(activeCount, formats.count(activeCount)),
            s.roomHeaderAgentCount(agentCount, formats.count(agentCount)),
            invitedCount > 0 ? s.panelInvitedCount(invitedCount, formats.count(invitedCount)) : null,
          ]
            .filter(Boolean)
            .join(Punct.metaSep)}
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
                {mine ? <span className="member-self-chip">{s.panelThisDevice}</span> : null}
              </div>
              <code className="member-id mono">{shortMemberId(member.identity_id)}</code>
            </div>
            {/* No `aria-label` here. It repeated verbatim the two strings the
                child spans already render visibly, on a generic `div` with no
                role — which most screen readers ignore outright, so it was
                markup that only ever cost maintenance. The dot+label pair
                below is the accessible rendering, and it already satisfies the
                never-colour-alone rule (docs/room-attention.md). */}
            <div className="member-badges">
              <span className={`member-role role-${member.role}`}>{rolePill(s, member.role)}</span>
              <span className={`member-status status-${tone}`}>
                <span className="dot" /> {memberStatus(s, member.status)}
              </span>
            </div>
            {canLeave ? (
              <button type="button" className="btn btn-sm btn-danger member-leave-btn" onClick={onLeaveRoom}>
                {s.panelLeave}
              </button>
            ) : null}
            {ownerCannotLeave ? (
              <span className="member-owner-note" title={s.panelOwnerStaysTitle}>
                {s.panelOwnerStays}
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
  const s = useStrings();
  const formats = useFormats();
  const agents = members.filter((m) => m.role === 'agent');
  if (agents.length === 0) {
    return <div className="panel-empty muted">{s.panelAgentsEmpty}</div>;
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
                  <span className="agent-label muted">{s.panelNoStatusPostedYet}</span>
                )}
              </div>
              {latest ? <time className="muted">{formats.clock(latest.ts)}</time> : null}
            </div>
            {latest?.status_message ? <p className="agent-msg">{latest.status_message}</p> : null}
            {typeof latest?.progress === 'number' ? (
              <div className="progress-row">
                <ProgressBar value={latest.progress} />
                <span className="progress-num">{formats.percent(Math.max(0, Math.min(100, latest.progress)))}</span>
              </div>
            ) : null}
            <div className="agent-foot muted">{s.panelAgentStatusFooter(memberStatus(s, agent.status))}</div>
          </div>
        );
      })}
    </div>
  );
}

// -- Files ---------------------------------------------------------------------

function mimeTypeLabel(s: Catalog, mime: string, fallback: string): string {
  if (!mime) return fallback || s.panelKindFile;
  const [type, subtype] = mime.split('/');
  if (!subtype) return mime;
  if (subtype === 'octet-stream') return fallback || s.panelKindBinary;
  if (type === 'text' && subtype === 'plain') return s.panelKindText;
  return subtype.replace(/[.+-]/g, ' ');
}

function fileTypeLabel(s: Catalog, file: FileEntry, ext: string): string {
  return mimeTypeLabel(s, file.mime, ext);
}

function selectedFileTypeLabel(s: Catalog, file: File): string {
  const ext = extOf(file.name).toLowerCase();
  return mimeTypeLabel(s, file.type, ext);
}

function FilesTab({
  files,
  selfId,
  fetches,
  selectedFileId = null,
  onSelectFile,
  onFetch,
  onRecheckFiles,
  onSharePath,
  onShareBrowserFile,
}: {
  files: FileEntry[];
  selfId: string | null;
  fetches: Record<string, FetchState>;
  /** The file the route deep-links to, or null (#67). Its row is the one the
   *  contextual inspector opens on — route state, never held here. */
  selectedFileId?: string | null;
  /** Select a file row (navigates to `/rooms/:id/files/:fileId`) or null to
   *  deselect (back to the list). Toggling is the caller's job to express as a
   *  route; this tab only asks for it. */
  onSelectFile(fileId: string | null): void;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onSharePath(path: string): Promise<void>;
  onShareBrowserFile(file: File): Promise<void>;
}) {
  const s = useStrings();
  const formats = useFormats();
  const [path, setPath] = useState('');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [pickerKey, setPickerKey] = useState(0);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [shareError, setShareError] = useState<DaemonErrorShape | null>(null);

  // A deep link (or an "Open in Files" from the timeline) can name a file far
  // down the list: bring the selected row on screen. Keyed on the selection so
  // a manual scroll away is never yanked back, and it never fires twice for the
  // same id. If the id isn't in `files` yet the effect re-runs when they arrive;
  // an id that is genuinely absent (once the list is in) is consumed so a dead
  // deep link cannot loop.
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const lastScrolled = useRef<string | null>(null);
  useEffect(() => {
    if (selectedFileId === null) {
      lastScrolled.current = null;
      return;
    }
    if (selectedFileId === lastScrolled.current) return;
    const row = rowRefs.current.get(selectedFileId);
    if (row) {
      // Reduced motion is a contract, not a hint (CONTRIBUTING.md) — the shared
      // helper is the only place that decides `behavior`.
      scrollIntoView(row);
      // Land focus on the row the deep link named, exactly as the Pipes tool
      // does. Scrolling a row into view while leaving focus behind was two
      // behaviours for one navigation contract.
      row.querySelector<HTMLElement>('.file-row-select')?.focus({ preventScroll: true });
      lastScrolled.current = selectedFileId;
    } else if (files.length > 0 && !files.some((f) => f.file_id === selectedFileId)) {
      lastScrolled.current = selectedFileId;
    }
  }, [selectedFileId, files]);

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
      ? s.panelFilesHeroEmptyDetail
      : [
          s.panelFilesHeroDetail(formats.bytes(totalBytes), availableCount, formats.count(availableCount)),
          fetchedCount > 0 ? s.panelNFetched(fetchedCount, formats.count(fetchedCount)) : null,
          servingCount > 0 ? s.panelServedByYou(servingCount, formats.count(servingCount)) : null,
        ]
          .filter(Boolean)
          .join(Punct.metaSep);
  const shareHelpId = 'file-share-help';
  const selectedType = selectedFile ? selectedFileTypeLabel(s, selectedFile) : null;

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
      {/* Plain section — see the members summary above on why this is not a
          labelled region. */}
      <section className={`files-hero${files.length === 0 ? ' is-empty' : ''}`}>
        <span className="files-hero-mark" aria-hidden="true">
          ▤
        </span>
        <div className="files-hero-copy">
          <h2>
            {files.length === 0
              ? s.panelNoSharedFilesYet
              : s.panelSharedFileCount(files.length, formats.count(files.length))}
          </h2>
          <p>{heroDetail}</p>
        </div>
        {files.length > 0 ? (
          <dl className="files-stats" aria-label={s.panelFileAvailabilityLabel}>
            <div>
              <dt>{s.panelFetchableNow}</dt>
              <dd>
                {s.panelFetchableNowValue(
                  availableCount,
                  files.length,
                  formats.count(availableCount),
                  formats.count(files.length),
                )}
              </dd>
            </div>
            <div>
              <dt>{s.panelProviderDevices}</dt>
              <dd>{formats.count(providerCount)}</dd>
            </div>
          </dl>
        ) : null}
      </section>

      {/* List-first (#67): sharing is a compact affordance that reveals the
          picker, not a form standing open above the list. The list below is
          what the panel is *for*; the sheet is one click away when it's needed. */}
      <div className="files-share-bar">
        <button
          type="button"
          className="btn btn-primary file-share-toggle"
          aria-expanded={pickerOpen}
          aria-controls="file-share-sheet"
          onClick={() => setPickerOpen((open) => !open)}
        >
          {pickerOpen ? s.panelFilesShareToggleClose : s.panelFilesShareToggle}
        </button>
      </div>

      {pickerOpen ? (
        <form
          id="file-share-sheet"
          className="panel-form file-share-card"
          onSubmit={(e) => {
            e.preventDefault();
            void share();
          }}
        >
          <div className="panel-form-head">
            <div>
              <h2>{s.panelShareCardTitle}</h2>
              <p className="muted" id={shareHelpId}>
                {s.panelShareCardHelp}
              </p>
            </div>
            <span className="file-share-badge" aria-label={s.panelHashCheckedBadgeLabel}>
              {s.panelHashCheckedBadge}
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
              aria-label={s.panelChooseFileToShare}
              aria-describedby={shareHelpId}
            />
            {selectedFile ? (
              <div className="selected-file-card" aria-live="polite">
                <span className="selected-file-icon" aria-hidden="true">
                  {extOf(selectedFile.name).toUpperCase().slice(0, 4) || s.commonFileExtFallback.toUpperCase()}
                </span>
                <div className="selected-file-info">
                  <strong>{selectedFile.name}</strong>
                  <span className="muted">
                    {formats.bytes(selectedFile.size)}
                    {Punct.metaSep}
                    {selectedType}
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
                  {s.panelClearSelectedFile}
                </button>
              </div>
            ) : (
              <p className="file-picker-empty muted">{s.panelNoFileSelectedYet}</p>
            )}
          </div>

          <button
            type="submit"
            className="btn btn-primary file-share-submit"
            disabled={sharing || (!selectedFile && !path.trim())}
          >
            {sharing ? s.panelSharing : s.panelShare}
          </button>

          <details className="file-advanced-path">
            <summary>{s.panelAdvancedPathSummary}</summary>
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
                placeholder={s.panelPathPlaceholder}
                aria-label={s.panelPathFieldLabel}
                spellCheck={false}
              />
            </div>
            <p className="form-hint muted">{s.panelPathHint}</p>
          </details>
          <ErrorNote error={shareError} />
        </form>
      ) : null}

      {files.length > 0 ? (
        <div className="files-section-head">
          <h3>{s.panelSharedInThisRoom}</h3>
          <span>
            {availableCount === files.length
              ? s.panelAllFetchable
              : [
                  servingCount > 0 ? s.panelServedByYou(servingCount, formats.count(servingCount)) : null,
                  waitingProviderCount > 0
                    ? s.panelAwaitingProvider(waitingProviderCount, formats.count(waitingProviderCount))
                    : null,
                ]
                  .filter(Boolean)
                  .join(Punct.metaSep)}
          </span>
        </div>
      ) : null}

      {files.map((file) => {
        const tint = fileTint(file.name);
        const rawExt = extOf(file.name).toUpperCase();
        const ext = rawExt || s.commonFileExtFallback.toUpperCase();
        const type = fileTypeLabel(s, file, rawExt.toLowerCase());
        const mine = selfId !== null && file.sender_id === selfId;
        const fetchState = fetches[file.file_id];
        const availability: FetchAvailability = {
          available: file.available,
          providers: file.providers,
        };
        const fetched = fetchState?.phase === 'verified' || fetchState?.phase === 'fetched' || file.fetched;
        const failedState = fetchState?.phase === 'error' ? fetchState : null;
        const selected = selectedFileId === file.file_id;
        // `available` is "another provider device is a connected peer right now"
        // (the daemon excludes THIS device), so a file you shared reads as
        // not-available from your own view even though peers can fetch it. Label
        // by ownership so that is never shown as a fault. See list_files() in
        // crates/jeliya-core/src/supervisor.rs.
        const health = mine
          ? { tone: 'self', text: s.panelHealthServingToPeers }
          : fetched
            ? { tone: 'ok', text: s.panelHealthFetchedLocally }
            : failedState
              ? {
                  tone: 'warn',
                  text:
                    failedState.error.code === 'hash_mismatch'
                      ? s.panelHealthSecurityCheckFailed
                      : s.panelHealthFetchFailed,
                }
              : file.available
                ? { tone: 'ok', text: s.panelHealthReadyToFetch }
                : { tone: 'warn', text: s.commonNoProviderOnline };
        const providerText = s.panelNProviders(file.providers, formats.count(file.providers));
        return (
          <div
            key={file.file_id}
            ref={(el) => {
              if (el) rowRefs.current.set(file.file_id, el);
              else rowRefs.current.delete(file.file_id);
            }}
            className={`file-row${!file.available && !mine && !fetched ? ' unavailable' : ''}${
              selected ? ' file-row-selected' : ''
            }`}
            title={file.file_id}
          >
            {/* Selecting a row opens its inspector at `/rooms/:id/files/:fileId`;
                the SenderName rename button stays outside this control so a
                button never nests inside a button. */}
            <button
              type="button"
              className="file-row-select"
              aria-expanded={selected}
              onClick={() => onSelectFile(selected ? null : file.file_id)}
            >
              <span className="file-icon" style={{ background: `${tint}22`, color: tint }} aria-hidden="true">
                {ext.slice(0, 4)}
              </span>
              <span className="file-row-info">
                <span className="file-title-row">
                  <strong>{file.name}</strong>
                  <span className="file-kind">{type}</span>
                </span>
                <span className={`file-health ${health.tone}`}>
                  <span className="dot" />
                  {health.text}
                  {Punct.metaSep}
                  {providerText}
                </span>
              </span>
              <span className="file-row-caret" aria-hidden="true">
                {selected ? Glyph.disclosureOpen : Glyph.disclosureClosed}
              </span>
            </button>
            <div className="file-meta muted">
              {formats.bytes(file.size)}
              {Punct.metaSep}
              <SenderName id={file.sender_id} />
              {Punct.metaSep}
              {formats.clock(file.ts)}
            </div>
            {/* The contextual inspector: the selected file's fetch controls,
                honest state, and detail — the accounting unchanged, just moved
                behind selection so the list reads as a list. */}
            {selected ? (
              <div className="file-inspector">
                {mine ? (
                  <span className="file-self-note" title={s.commonServingTooltip}>
                    {s.commonServing}
                  </span>
                ) : (
                  <>
                    <FetchControl
                      state={fetchState}
                      availability={availability}
                      fileName={file.name}
                      onFetch={() => onFetch(file.file_id)}
                      onRecheck={onRecheckFiles}
                    />
                    <FetchDetail state={fetchState} />
                  </>
                )}
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

// -- Pipes ---------------------------------------------------------------------

/** Pipe state is a protocol value. Known states get display copy; a state from
 *  a newer daemon stays visible verbatim rather than being mislabeled. */
function pipeStateLabel(s: Catalog, state: string, connected: boolean): string {
  if (state === 'open') return connected ? s.pipeStateConnected : s.pipeStateOpen;
  if (state === 'closed') return s.pipeStateClosed;
  return state;
}

function PipesTab({
  pipes,
  members,
  selfId,
  conns,
  closing,
  selectedPipeId = null,
  onSelectPipe,
  onConnect,
  onClose,
  onExpose,
}: {
  pipes: PipeEntry[];
  members: Member[];
  selfId: string | null;
  conns: Record<string, PipeConnState>;
  closing: Set<string>;
  /** The pipe the route deep-links to, or null (#67) — route state, not held
   *  here. It drives selection, and the flash/scroll that identifies the row a
   *  timeline "Open in Pipes" referred to. */
  selectedPipeId?: string | null;
  /** Select a pipe row (navigates to `/rooms/:id/pipes/:pipeId`) or null to
   *  deselect. */
  onSelectPipe(pipeId: string | null): void;
  onConnect(pipeId: string): void;
  onClose(pipeId: string): void;
  onExpose(target: string, peerIdentity: string): Promise<void>;
}) {
  const s = useStrings();
  const names = useNames();
  const [target, setTarget] = useState('');
  const peerChoices = members.filter((m) => m.identity_id !== selfId);
  const [peer, setPeer] = useState('');
  const [exposing, setExposing] = useState(false);
  const [exposeError, setExposeError] = useState<DaemonErrorShape | null>(null);

  // The selected pipe (route state) is identified on arrival: focus its row and
  // flash it so a deep link — "Open in Pipes" from the timeline, or a pasted
  // URL — lands somewhere visible instead of appearing to change nothing (the
  // row may be far down). `lastFlashed` fires the flash once per selection: if
  // the id isn't in `pipes` yet the effect re-runs when they arrive; an id
  // genuinely absent once the list is in is consumed so a dead deep link can't
  // loop.
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const [flashPipeId, setFlashPipeId] = useState<string | null>(null);
  const lastFlashed = useRef<string | null>(null);
  useEffect(() => {
    if (selectedPipeId === null) {
      lastFlashed.current = null;
      return;
    }
    if (selectedPipeId === lastFlashed.current) return;
    const row = rowRefs.current.get(selectedPipeId);
    if (row) {
      scrollIntoView(row);
      // Focus the row's own button rather than the row box: programmatic focus
      // on a `tabindex="-1"` div does not match `:focus-visible`, so the global
      // focus ring never rendered and the only landing cue was the 1.6s flash.
      // A real control keeps the browser's keyboard-vs-pointer heuristic, so a
      // keyboard user arriving from "Open in Pipes" gets a persistent ring.
      row.querySelector<HTMLElement>('.pipe-row-head')?.focus({ preventScroll: true });
      setFlashPipeId(selectedPipeId);
      lastFlashed.current = selectedPipeId;
    } else if (pipes.length > 0 && !pipes.some((p) => p.pipe_id === selectedPipeId)) {
      lastFlashed.current = selectedPipeId;
    }
  }, [selectedPipeId, pipes]);
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
    <div className="panel-list pipes-panel">
      {/* Action-first (#67): the expose form is anchored at the top, reachable
          without scrolling past every existing pipe. The live pipes remain
          below it — hoisting the action never hides the state. */}
      <form
        className="panel-form pipe-expose"
        onSubmit={(e) => {
          e.preventDefault();
          void expose();
        }}
      >
        <h2>{s.panelExposeTitle}</h2>
        <p className="muted">{s.panelExposeCopy}</p>
        <div className="form-row">
          <input
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder={Example.pipeTarget}
            aria-label={s.panelTargetFieldLabel}
            spellCheck={false}
          />
        </div>
        <div className="form-row">
          <select
            value={peer || peerChoices[0]?.identity_id || ''}
            onChange={(e) => setPeer(e.target.value)}
            aria-label={s.panelAuthorizedPeerLabel}
          >
            {peerChoices.length === 0 ? <option value="">{s.panelNoOtherMembers}</option> : null}
            {peerChoices.map((m) => (
              <option key={m.identity_id} value={m.identity_id}>
                {s.panelPeerChoice(names.display(m.identity_id), roleInline(s, m.role))}
              </option>
            ))}
          </select>
          <button type="submit" className="btn" disabled={exposing || !target.trim() || peerChoices.length === 0}>
            {exposing ? s.panelExposing : s.panelExpose}
          </button>
        </div>
        <ErrorNote error={exposeError} />
      </form>

      {pipes.length === 0 ? <div className="panel-empty muted">{s.panelPipesEmpty}</div> : null}
      {pipes.map((pipe) => {
        const conn = conns[pipe.pipe_id];
        const selected = selectedPipeId === pipe.pipe_id;
        return (
          <div
            key={pipe.pipe_id}
            ref={(el) => {
              if (el) rowRefs.current.set(pipe.pipe_id, el);
              else rowRefs.current.delete(pipe.pipe_id);
            }}
            className={`pipe-row${pipe.state === 'closed' ? ' closed' : ''}${
              flashPipeId === pipe.pipe_id ? ' pipe-row-flash' : ''
            }${selected ? ' pipe-row-selected' : ''}`}
          >
            {/* Selecting a pipe opens its inspector at `/rooms/:id/pipes/:pipeId`.
                The SenderName rename buttons and the connect/close actions stay
                outside this control so no button nests inside a button. */}
            <button
              type="button"
              className="pipe-row-head"
              aria-expanded={selected}
              onClick={() => onSelectPipe(selected ? null : pipe.pipe_id)}
            >
              <span className="pipe-icon" aria-hidden="true">
                {Glyph.pipe}
              </span>
              <strong className="mono">{pipe.target}</strong>
              {/* Pipe connection, and only that (decision 4): exposed with a
                  live forwarding session, exposed with none, or closed. */}
              <span className={`chip chip-state state-${pipe.state}`}>
                {pipeStateLabel(s, pipe.state, pipe.connected)}
              </span>
            </button>
            <div className="pipe-row-meta muted">
              <Template
                template={s.panelPipeMeta}
                slots={{
                  openedBy: <SenderName id={pipe.opened_by} />,
                  authorized: pipe.authorized_peer ? <SenderName id={pipe.authorized_peer} /> : Punct.missingValue,
                }}
              />
            </div>
            {pipe.state === 'open' ? (
              <div className="pipe-row-actions">
                {!conn || conn.phase === 'error' ? (
                  <button type="button" className="btn btn-sm" onClick={() => onConnect(pipe.pipe_id)}>
                    {s.panelConnect}
                  </button>
                ) : conn.phase === 'connecting' ? (
                  <button type="button" className="btn btn-sm" disabled>
                    <span className="spinner" aria-hidden="true" /> {s.panelConnecting}
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
                      {s.panelOpenPreview}
                    </a>
                  </>
                )}
                {closing.has(pipe.pipe_id) ? (
                  <button type="button" className="btn btn-sm btn-ghost" disabled>
                    <span className="spinner" aria-hidden="true" /> {s.panelClosingPipe}
                  </button>
                ) : (
                  <button type="button" className="btn btn-sm btn-ghost" onClick={() => onClose(pipe.pipe_id)}>
                    {s.panelClosePipe}
                  </button>
                )}
              </div>
            ) : null}
            {conn?.phase === 'error' ? <ErrorNote error={conn.error} /> : null}
          </div>
        );
      })}
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
  selectedItem = null,
  onSelectItem,
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
  isPage,
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
  /** The file/pipe the route deep-links to, or null (#67). The active tool
   *  reads it as its selected file (files) or pipe (pipes) id. */
  selectedItem?: string | null;
  /** Select a file/pipe within the active tool (or null to deselect); the
   *  parent turns it into a route so the selection is a deep link. */
  onSelectItem(itemId: string | null): void;
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
  /** True when this panel is the only live surface — the whole screen on
   *  compact, a drawer over an inert workspace on medium. It then carries the
   *  page's `main` landmark and its `h1` (the room name, whose `h1` in the
   *  workspace header is out of the accessibility tree in both cases). On wide
   *  it opens beside a live workspace and stays `complementary`. */
  isPage: boolean;
}) {
  const s = useStrings();
  // Named for the tool it is showing, so the landmark's name changes with the
  // route and never collides with the room rail's (issue #72).
  const panelName = s.panelInspectorLabel(roomDestLabel(s, tab));
  const RoomHeading = isPage ? 'h1' : 'div';
  return (
    // A plain `div` with an explicit role rather than an `<aside>` — see the
    // room rail: overriding a landmark element's implicit role trips
    // `aria-allowed-role`, and switching the element between shells would
    // remount the tool and lose its scroll position.
    <div
      className="right-panel"
      id="room-inspector"
      role={isPage ? 'main' : 'complementary'}
      aria-label={isPage ? undefined : panelName}
    >
      {/* The room stays named on every room-scoped surface (decision 3). On
          compact this panel IS the screen and the room header is off in the
          hidden `.center` pane; on medium it floats over a workspace made
          inert. Not aria-hidden — in both cases this is the only place the room
          name reaches the accessible tree, which is why it is the `h1` there. */}
      <div className="panel-head">
        {/* One navigation, three affordances. Compact needs a Back of its own:
            this panel is the whole screen there and the bottom bar is gone, so
            without it the only way out would be the browser's own Back — which
            an installed PWA does not show. */}
        {shell === 'compact' ? (
          <button type="button" className="icon-btn panel-back" onClick={onClose} aria-label={s.roomBackToActivity}>
            <span aria-hidden="true">{Glyph.back}</span>
          </button>
        ) : null}
        {roomName ? <RoomHeading className="panel-room-context">{roomName}</RoomHeading> : <span />}
        {shell !== 'compact' ? (
          <button type="button" className="icon-btn panel-close" onClick={onClose} aria-label={s.roomCloseInspector}>
            <span aria-hidden="true">{Glyph.close}</span>
          </button>
        ) : null}
      </div>
      {/* Whenever this panel covers the workspace — the whole pane on compact,
          a drawer on medium — it owes the room's nav, because the workspace's
          copy is underneath it. Only on wide, where the panel opens beside the
          workspace rather than over it, does the workspace keep the strip and
          this render nothing: two tablists for one room would be two tablists
          in the a11y tree. */}
      {shell !== 'wide' ? <RoomNav dest={tab} counts={counts} onDest={onDest} controlsId="panel-body" /> : null}
      <div className="panel-body" id="panel-body" role="tabpanel" aria-labelledby={`room-tab-${tab}`}>
        {tab === 'people' ? <MembersTab members={members} selfId={selfId} onLeaveRoom={onLeaveRoom} /> : null}
        {tab === 'agents' ? <AgentsTab members={members} timeline={timeline} /> : null}
        {tab === 'files' ? (
          <FilesTab
            files={files}
            selfId={selfId}
            fetches={fetches}
            selectedFileId={selectedItem}
            onSelectFile={onSelectItem}
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
            selectedPipeId={selectedItem}
            onSelectPipe={onSelectItem}
            onConnect={onPipeConnect}
            onClose={onPipeClose}
            onExpose={onPipeExpose}
          />
        ) : null}
      </div>
    </div>
  );
}
