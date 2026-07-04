import { useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import type { DaemonErrorShape, FileEntry, Member, PipeEntry, TimelineEvent } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { extOf, fileTint, formatBytes, formatTime, labelTone, prettyLabel } from '../lib/format';
import { useNames } from './names';
import { Avatar, ErrorNote, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchState } from './ui';

export type PanelTab = 'agents' | 'files' | 'pipes';

export type PipeConnState =
  | { phase: 'connecting' }
  | { phase: 'connected'; local_addr: string }
  | { phase: 'error'; error: DaemonErrorShape };

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

function FilesTab({
  files,
  fetches,
  onFetch,
  onShare,
}: {
  files: FileEntry[];
  fetches: Record<string, FetchState>;
  onFetch(fileId: string): void;
  onShare(path: string): Promise<void>;
}) {
  const [path, setPath] = useState('');
  const [sharing, setSharing] = useState(false);
  const [shareError, setShareError] = useState<DaemonErrorShape | null>(null);

  const share = async () => {
    const p = path.trim();
    if (!p || sharing) return;
    setSharing(true);
    setShareError(null);
    try {
      await onShare(p);
      setPath('');
    } catch (e) {
      setShareError(errorShape(e));
    } finally {
      setSharing(false);
    }
  };

  return (
    <div className="panel-list">
      {files.length === 0 ? (
        <div className="panel-empty muted">No files yet — share one below; peers fetch it over P2P.</div>
      ) : null}
      {files.map((file) => {
        const tint = fileTint(file.name);
        const ext = extOf(file.name).toUpperCase() || 'FILE';
        return (
          <div key={file.file_id} className="file-row">
            <div className="file-row-main">
              <span className="file-icon" style={{ background: `${tint}22`, color: tint }}>
                {ext.slice(0, 4)}
              </span>
              <div className="file-row-info">
                <strong>{file.name}</strong>
                <span className="muted">
                  {formatBytes(file.size)} · <SenderName id={file.sender_id} /> · {formatTime(file.ts)}
                </span>
                <span className={`file-avail ${file.available ? 'ok' : 'warn'}`}>
                  <span className="dot" />
                  {file.available ? 'Available' : 'Unavailable'} · {file.providers} provider
                  {file.providers === 1 ? '' : 's'}
                </span>
              </div>
              <FetchControl state={fetches[file.file_id]} onFetch={() => onFetch(file.file_id)} />
            </div>
            <FetchDetail state={fetches[file.file_id]} />
          </div>
        );
      })}

      <form
        className="panel-form"
        onSubmit={(e) => {
          e.preventDefault();
          void share();
        }}
      >
        <h2>Share a file</h2>
        <p className="muted">Path on this machine — the daemon imports it into the blob store.</p>
        <div className="form-row">
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/path/to/file.pdf"
            aria-label="File path to share"
            spellCheck={false}
          />
          <button type="submit" className="btn" disabled={sharing || !path.trim()}>
            {sharing ? 'Sharing…' : 'Share'}
          </button>
        </div>
        <ErrorNote error={shareError} />
      </form>
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
  onConnect,
  onClose,
  onExpose,
}: {
  pipes: PipeEntry[];
  members: Member[];
  selfId: string | null;
  conns: Record<string, PipeConnState>;
  closing: Set<string>;
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
          <div key={pipe.pipe_id} className={`pipe-row${pipe.state === 'closed' ? ' closed' : ''}`}>
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
              {names.isSelf(pipe.authorized_peer) ? 'You' : <SenderName id={pipe.authorized_peer} />}
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
  onTab,
  members,
  timeline,
  files,
  pipes,
  selfId,
  fetches,
  onFetch,
  onShareFile,
  pipeConns,
  closingPipes,
  onPipeConnect,
  onPipeClose,
  onPipeExpose,
}: {
  tab: PanelTab;
  onTab(tab: PanelTab): void;
  members: Member[];
  timeline: TimelineEvent[];
  files: FileEntry[];
  pipes: PipeEntry[];
  selfId: string | null;
  fetches: Record<string, FetchState>;
  onFetch(fileId: string): void;
  onShareFile(path: string): Promise<void>;
  pipeConns: Record<string, PipeConnState>;
  closingPipes: Set<string>;
  onPipeConnect(pipeId: string): void;
  onPipeClose(pipeId: string): void;
  onPipeExpose(target: string, peerIdentity: string): Promise<void>;
}) {
  const agentCount = members.filter((m) => m.role === 'agent').length;
  const openPipes = pipes.filter((p) => p.state === 'open').length;

  const tabs: { id: PanelTab; label: string; count: number }[] = [
    { id: 'agents', label: 'Agents', count: agentCount },
    { id: 'files', label: 'Files', count: files.length },
    { id: 'pipes', label: 'Live Pipes', count: openPipes },
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
            {t.label} <span className="count">{t.count}</span>
          </button>
        ))}
      </div>
      <div className="panel-body" id="panel-body" role="tabpanel" aria-labelledby={`panel-tab-${tab}`}>
        {tab === 'agents' ? <AgentsTab members={members} timeline={timeline} /> : null}
        {tab === 'files' ? <FilesTab files={files} fetches={fetches} onFetch={onFetch} onShare={onShareFile} /> : null}
        {tab === 'pipes' ? (
          <PipesTab
            pipes={pipes}
            members={members}
            selfId={selfId}
            conns={pipeConns}
            closing={closingPipes}
            onConnect={onPipeConnect}
            onClose={onPipeClose}
            onExpose={onPipeExpose}
          />
        ) : null}
      </div>
    </aside>
  );
}
