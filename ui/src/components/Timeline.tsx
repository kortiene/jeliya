import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { DaemonErrorShape, FileEntry, FileRef, TimelineEvent } from '../lib/protocol';
import { dayLabel, extOf, fileTint, formatBytes, formatTime, labelTone, prettyLabel, shortId } from '../lib/format';
import { Avatar, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchAvailability, FetchState } from './ui';

export interface PendingMessage {
  clientId: string;
  body: string;
  ts: number;
  phase: 'sending' | 'syncing' | 'failed';
  eventId?: string;
  error?: DaemonErrorShape;
}

type TimelineItem = { type: 'event'; event: TimelineEvent } | { type: 'pending'; pending: PendingMessage };
type TimelineSide = 'own' | 'remote' | 'system';

const GROUP_WINDOW_MS = 5 * 60 * 1000;

function FileTile({
  file,
  isSelfOwned,
  state,
  availability,
  onFetch,
  onRecheckFiles,
}: {
  file: FileRef;
  isSelfOwned: boolean;
  state?: FetchState;
  availability?: FetchAvailability;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
}) {
  const tint = fileTint(file.name);
  const ext = extOf(file.name).toUpperCase() || 'FILE';
  return (
    <div className="file-tile-wrap">
      <div className="file-tile">
        <span className="file-icon" style={{ background: `${tint}22`, color: tint }}>
          {ext.slice(0, 4)}
        </span>
        <span className="file-tile-info">
          <strong>{file.name}</strong>
          <span className="muted">
            {formatBytes(file.size)} · {ext}
          </span>
        </span>
        {isSelfOwned ? (
          <span className="file-self-note" title="This daemon is already serving this file to peers.">
            Serving
          </span>
        ) : (
          <FetchControl
            state={state}
            availability={availability}
            availabilityPending={!availability}
            onFetch={() => onFetch(file.file_id)}
            onRecheck={onRecheckFiles}
          />
        )}
      </div>
      {isSelfOwned ? null : <FetchDetail state={state} />}
    </div>
  );
}

function EventCard({
  event,
  files,
  fetches,
  selfId,
  compact,
  onFetch,
  onRecheckFiles,
  onShowPipes,
}: {
  event: TimelineEvent;
  files: FileEntry[];
  fetches: Record<string, FetchState>;
  selfId: string | null;
  compact: boolean;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onShowPipes(): void;
}) {
  const senderId = event.sender.identity_id;
  const time = formatTime(event.ts);
  const isOwn = selfId !== null && senderId === selfId;

  switch (event.kind) {
    case 'room_created':
      return (
        <div className="sysline">
          <SenderName id={senderId} /> created the room · {time}
        </div>
      );

    case 'member_invited': {
      const invitee = event.member?.identity_id;
      return (
        <div className="sysline">
          <SenderName id={senderId} /> invited{' '}
          {invitee ? <SenderName id={invitee} /> : 'someone'} as {event.member?.role ?? 'member'} · {time}
        </div>
      );
    }

    case 'member_joined': {
      const who = event.member?.identity_id ?? senderId;
      return (
        <div className="sysline">
          <SenderName id={who} /> joined as {event.member?.role ?? event.sender.role} · {time}
        </div>
      );
    }

    case 'member_left': {
      const who = event.member?.identity_id ?? senderId;
      return (
        <div className="sysline">
          <SenderName id={who} /> left the room · {time}
        </div>
      );
    }

    case 'message': {
      return (
        <div className={`msg-row${isOwn ? ' own' : ''}${compact ? ' compact' : ''}`}>
          {isOwn ? null : compact ? <span className="avatar-spacer" aria-hidden="true" /> : <Avatar id={senderId} />}
          <div className="msg-col">
            {!compact ? (
              <div className="msg-meta">
                <SenderName id={senderId} className="msg-sender" />
                {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
                <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
              </div>
            ) : null}
            <div className="msg-bubble">{event.body}</div>
          </div>
        </div>
      );
    }

    case 'agent_status': {
      const label = event.label ?? 'status';
      const artifacts = event.artifacts ?? [];
      return (
        <div className={`event-card agent-work-card${isOwn ? ' own' : ''}`}>
          {isOwn ? null : <Avatar id={senderId} />}
          <div className="event-main">
            <div className="event-head">
              <SenderName id={senderId} className="event-sender" />
              <span className="chip chip-role">AGENT</span>
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
              <span className={`chip chip-label tone-${labelTone(label)}`}>{prettyLabel(label)}</span>
            </div>
            <div className="agent-work-title">
              <span aria-hidden="true">✦</span>
              <strong>{prettyLabel(label)}</strong>
            </div>
            {event.status_message ? <p className="event-text">{event.status_message}</p> : null}
            {typeof event.progress === 'number' ? (
              <div className="progress-row">
                <ProgressBar value={event.progress} />
                <span className="progress-num">{Math.max(0, Math.min(100, event.progress))}%</span>
              </div>
            ) : null}
            {artifacts.length > 0 ? (
              <div className="artifact-row">
                {artifacts.map((fileId) => {
                  const file = files.find((f) => f.file_id === fileId);
                  return (
                    <span key={fileId} className="chip chip-artifact" title={fileId}>
                      <span aria-hidden="true">⎘</span>
                      {file ? file.name : shortId(fileId)}
                    </span>
                  );
                })}
              </div>
            ) : null}
          </div>
        </div>
      );
    }

    case 'file_shared': {
      if (!event.file) return null;
      const fileEntry = files.find((f) => f.file_id === event.file?.file_id);
      return (
        <div className={`event-card${isOwn ? ' own' : ''}`}>
          {isOwn ? null : <Avatar id={senderId} />}
          <div className="event-main">
            <div className="event-head">
              <SenderName id={senderId} className="event-sender" />
              {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
              <span className="muted">shared a file</span>
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
            </div>
            <FileTile
              file={event.file}
              isSelfOwned={isOwn}
              state={fetches[event.file.file_id]}
              availability={
                fileEntry ? { available: fileEntry.available, providers: fileEntry.providers } : undefined
              }
              onFetch={onFetch}
              onRecheckFiles={onRecheckFiles}
            />
          </div>
        </div>
      );
    }

    case 'pipe_opened': {
      if (!event.pipe) return null;
      return (
        <div className={`event-card${isOwn ? ' own' : ''}`}>
          {isOwn ? null : <Avatar id={senderId} />}
          <div className="event-main">
            <div className="event-head">
              <SenderName id={senderId} className="event-sender" />
              {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
              <span className="muted">opened a pipe</span>
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
            </div>
            <div className="pipe-tile">
              <span className="pipe-icon" aria-hidden="true">
                ⤳
              </span>
              <span className="file-tile-info">
                <strong className="mono">{event.pipe.target ?? '—'}</strong>
                <span className="muted">
                  authorized peer:{' '}
                  {event.pipe.authorized_peer ? <SenderName id={event.pipe.authorized_peer} /> : '—'}
                </span>
              </span>
              <button type="button" className="btn btn-sm" onClick={onShowPipes}>
                Open in Pipes
              </button>
            </div>
          </div>
        </div>
      );
    }

    case 'pipe_closed':
      return (
        <div className="sysline">
          <SenderName id={senderId} /> closed pipe <code className="mono">{event.pipe?.target ?? ''}</code> · {time}
        </div>
      );

    default:
      return null;
  }
}

function PendingMessageCard({
  message,
  compact,
  onRetry,
}: {
  message: PendingMessage;
  compact: boolean;
  onRetry(clientId: string): void;
}) {
  const label =
    message.phase === 'failed'
      ? "Couldn't send"
      : message.phase === 'syncing'
        ? 'Sent locally, syncing...'
        : 'Sending...';
  return (
    <div className={`msg-row own pending ${message.phase}${compact ? ' compact' : ''}`}>
      <div className="msg-col">
        {!compact ? (
          <div className="msg-meta pending-meta">
            <span>You</span>
            <time dateTime={new Date(message.ts).toISOString()}>{formatTime(message.ts)}</time>
          </div>
        ) : null}
        <div className="msg-bubble">{message.body}</div>
        <div className="pending-line">
          {message.phase !== 'failed' ? <span className="spinner" aria-hidden="true" /> : null}
          <span>{label}</span>
          {message.phase === 'failed' ? (
            <button type="button" className="text-btn" onClick={() => onRetry(message.clientId)}>
              Retry
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function itemTs(item: TimelineItem): number {
  return item.type === 'event' ? item.event.ts : item.pending.ts;
}

function itemSender(item: TimelineItem, selfId: string | null): string | null {
  if (item.type === 'pending') return selfId;
  return item.event.kind === 'message' ? item.event.sender.identity_id : null;
}

function shouldGroup(prev: TimelineItem | null, item: TimelineItem, selfId: string | null): boolean {
  if (!prev) return false;
  const sender = itemSender(item, selfId);
  const prevSender = itemSender(prev, selfId);
  if (!sender || !prevSender || sender !== prevSender) return false;
  return itemTs(item) - itemTs(prev) <= GROUP_WINDOW_MS;
}

function itemSide(item: TimelineItem, selfId: string | null): TimelineSide {
  if (item.type === 'pending') return 'own';
  const { event } = item;
  if (!['message', 'agent_status', 'file_shared', 'pipe_opened'].includes(event.kind)) {
    return 'system';
  }
  return selfId !== null && event.sender.identity_id === selfId ? 'own' : 'remote';
}

export function Timeline({
  events,
  pendingMessages,
  files,
  fetches,
  loading,
  selfId,
  onFetch,
  onRecheckFiles,
  onRetryPendingMessage,
  onShowPipes,
}: {
  events: TimelineEvent[];
  pendingMessages: PendingMessage[];
  files: FileEntry[];
  fetches: Record<string, FetchState>;
  loading: boolean;
  selfId: string | null;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onRetryPendingMessage(clientId: string): void;
  onShowPipes(): void;
}) {
  const scroller = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);
  const previousItemCount = useRef(0);
  const [newItemCount, setNewItemCount] = useState(0);

  const items: TimelineItem[] = [
    ...events.map((event) => ({ type: 'event' as const, event })),
    ...pendingMessages.map((pending) => ({ type: 'pending' as const, pending })),
  ].sort((a, b) => itemTs(a) - itemTs(b));

  useEffect(() => {
    const el = scroller.current;
    const previous = previousItemCount.current;
    const delta = Math.max(0, items.length - previous);
    if (el) {
      if (stickToBottom.current) {
        el.scrollTop = el.scrollHeight;
        setNewItemCount(0);
      } else if (delta > 0) {
        setNewItemCount((count) => count + delta);
      }
    }
    previousItemCount.current = items.length;
  }, [items.length]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 140;
    if (stickToBottom.current) setNewItemCount(0);
  };

  const scrollToBottom = () => {
    const el = scroller.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    stickToBottom.current = true;
    setNewItemCount(0);
  };

  let lastDay = '';
  const rows: { key: string; node: ReactNode; side: TimelineSide; compact: boolean }[] = [];
  let prevItem: TimelineItem | null = null;
  for (const item of items) {
    const ts = itemTs(item);
    const day = dayLabel(ts);
    if (day !== lastDay) {
      lastDay = day;
      rows.push({
        key: `day-${day}-${item.type === 'event' ? item.event.event_id : item.pending.clientId}`,
        side: 'system',
        compact: false,
        node: (
          <div className="day-divider">
            <span>{day}</span>
          </div>
        ),
      });
    }
    const compact = shouldGroup(prevItem, item, selfId);
    const side = itemSide(item, selfId);
    rows.push({
      key: item.type === 'event' ? item.event.event_id : item.pending.clientId,
      side,
      compact,
      node:
        item.type === 'event' ? (
          <EventCard
            event={item.event}
            files={files}
            fetches={fetches}
            selfId={selfId}
            compact={compact}
            onFetch={onFetch}
            onRecheckFiles={onRecheckFiles}
            onShowPipes={onShowPipes}
          />
        ) : (
          <PendingMessageCard message={item.pending} compact={compact} onRetry={onRetryPendingMessage} />
        ),
    });
    prevItem = item;
  }

  return (
    <div className="timeline-shell">
      <div
        className="timeline"
        ref={scroller}
        onScroll={onScroll}
        role="log"
        aria-label="Room timeline"
        aria-busy={loading}
      >
      {loading ? (
        <div aria-hidden="true">
          <div className="skel-row">
            <span className="skel-avatar skel" />
            <div className="skel-lines">
              <span className="skel-line skel" style={{ width: '32%' }} />
              <span className="skel-line skel" style={{ width: '74%' }} />
            </div>
          </div>
          <div className="skel-row">
            <span className="skel-avatar skel" />
            <div className="skel-lines">
              <span className="skel-line skel" style={{ width: '24%' }} />
              <span className="skel-line skel" style={{ width: '86%' }} />
              <span className="skel-line skel" style={{ width: '58%' }} />
            </div>
          </div>
          <div className="skel-row">
            <span className="skel-avatar skel" />
            <div className="skel-lines">
              <span className="skel-line skel" style={{ width: '40%' }} />
              <span className="skel-line skel" style={{ width: '66%' }} />
            </div>
          </div>
        </div>
      ) : null}
      {rows.map((row) => (
        <div
          key={row.key}
          className={`timeline-row side-${row.side}${row.compact ? ' is-compact' : ''}`}
        >
          {row.node}
        </div>
      ))}
      {!loading && items.length === 0 ? (
        <div className="timeline-empty muted">No events yet — say something below.</div>
      ) : null}
      </div>
      {newItemCount > 0 ? (
        <button type="button" className="new-messages" onClick={scrollToBottom}>
          {newItemCount} new message{newItemCount === 1 ? '' : 's'}
        </button>
      ) : null}
    </div>
  );
}
