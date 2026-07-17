import { useCallback, useEffect, useRef, useState } from 'react';
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

/** A deliberate reading position, persisted across the keyed remount when the
 *  user comes back to a room they had scrolled up in. `itemCount` is how many
 *  items they had actually seen — the difference against the reloaded backlog
 *  feeds the "N new messages" control instead of a silent jump. */
export interface TimelineView {
  scrollTop: number;
  itemCount: number;
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
  onShowPipes(pipeId?: string): void;
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
              <button type="button" className="btn btn-sm" onClick={() => onShowPipes(event.pipe?.pipe_id)}>
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
  savedView = null,
  onSaveView,
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
  savedView?: TimelineView | null;
  onSaveView?(view: TimelineView | null): void;
  onFetch(fileId: string): void;
  onRecheckFiles(): void;
  onRetryPendingMessage(clientId: string): void;
  onShowPipes(pipeId?: string): void;
}) {
  const scroller = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(savedView === null);
  const previousItemCount = useRef(0);
  const restorePending = useRef<TimelineView | null>(savedView);
  // Seen-count baseline across a wholesale backlog reload (openRoom clears
  // then refills the same room, e.g. on reconnect) — without it the whole
  // reloaded backlog would be announced as new activity.
  const reloadBaseline = useRef<number | null>(null);
  const lastScrollTop = useRef(savedView?.scrollTop ?? 0);
  const [newItemCount, setNewItemCount] = useState(0);

  // Render-time mirrors so the stable callbacks below never go stale.
  const loadingRef = useRef(loading);
  loadingRef.current = loading;
  const newItemCountRef = useRef(newItemCount);
  newItemCountRef.current = newItemCount;
  const onSaveViewRef = useRef(onSaveView);
  onSaveViewRef.current = onSaveView;
  const itemCountRef = useRef(0);

  const items: TimelineItem[] = [
    ...events.map((event) => ({ type: 'event' as const, event })),
    ...pendingMessages.map((pending) => ({ type: 'pending' as const, pending })),
  ].sort((a, b) => itemTs(a) - itemTs(b));
  // Updated during render, not in the effect: the ResizeObserver path can run
  // syncScroll between a commit and its effect, and must see this render's
  // count, never the previous one.
  itemCountRef.current = items.length;

  // The one place scroll position is written. Compact keeps inactive panes
  // display:none, which zeroes every measurement and (on reveal) wipes
  // scrollTop — so this must run not only when items change but whenever the
  // scroller is laid out again (the ResizeObserver below): first reveal of an
  // auto-opened room, hide/show cycles, rotation, and composer growth.
  const syncScroll = useCallback(() => {
    const el = scroller.current;
    if (!el || el.clientHeight === 0) return; // hidden — nothing measurable
    if (restorePending.current !== null) {
      if (loadingRef.current) return; // wait for the reloaded backlog
      const max = el.scrollHeight - el.clientHeight;
      el.scrollTop = Math.min(restorePending.current.scrollTop, Math.max(0, max));
      lastScrollTop.current = el.scrollTop;
      // Everything beyond what the reader had seen when they left is "new" —
      // counted here, once the backlog is really in, because the brief empty
      // render while room.open reloads must not be mistaken for churn.
      setNewItemCount(Math.max(0, itemCountRef.current - restorePending.current.itemCount));
      restorePending.current = null;
      return;
    }
    if (stickToBottom.current) {
      el.scrollTop = el.scrollHeight;
      lastScrollTop.current = el.scrollTop;
      setNewItemCount(0);
    } else if (el.scrollTop === 0 && lastScrollTop.current > 0) {
      // display:none reset the position under us — reinstate the reading spot.
      el.scrollTop = Math.min(lastScrollTop.current, el.scrollHeight - el.clientHeight);
    }
  }, []);

  useEffect(() => {
    const previous = previousItemCount.current;
    previousItemCount.current = items.length;
    if (!scroller.current) return;
    if (items.length === 0 && previous > 0 && loadingRef.current) {
      // openRoom emptied the backlog for a reload of the same room (e.g. a
      // reconnect re-running bootstrap). Remember how much the reader had
      // actually seen — announcing the whole reloaded backlog as new would
      // be a lie.
      reloadBaseline.current = previous - newItemCountRef.current;
    }
    // Live deltas only — while a restore is pending the backlog is being
    // reloaded wholesale and syncScroll derives the count from the saved view.
    if (!stickToBottom.current && restorePending.current === null) {
      if (reloadBaseline.current !== null) {
        if (items.length > 0) {
          setNewItemCount(Math.max(0, items.length - reloadBaseline.current));
          reloadBaseline.current = null;
        }
      } else {
        const delta = Math.max(0, items.length - previous);
        if (delta > 0) setNewItemCount((count) => count + delta);
      }
    } else {
      reloadBaseline.current = null;
    }
    syncScroll();
  }, [items.length, syncScroll]);

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const observer = new ResizeObserver(() => syncScroll());
    observer.observe(el);
    return () => observer.disconnect();
  }, [syncScroll]);

  // Persist a deliberate reading position across the keyed remount; a room
  // left at the bottom re-opens at its newest event instead.
  useEffect(() => {
    return () => {
      const save = onSaveViewRef.current;
      if (!save) return;
      if (stickToBottom.current) {
        save(null);
      } else if (restorePending.current !== null) {
        // Never revealed on this visit — carry the original view forward
        // untouched so the seen-count stays honest.
        save(restorePending.current);
      } else {
        save({
          scrollTop: lastScrollTop.current,
          itemCount: itemCountRef.current - newItemCountRef.current,
        });
      }
    };
  }, []);

  const onScroll = () => {
    const el = scroller.current;
    if (!el || el.clientHeight === 0) return;
    lastScrollTop.current = el.scrollTop;
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 140;
    if (stickToBottom.current) setNewItemCount(0);
  };

  const scrollToBottom = () => {
    const el = scroller.current;
    if (!el) return;
    // Reduced motion is a contract, not a hint (WCAG floor in CONTRIBUTING):
    // the jump to the newest event lands instantly instead of animating.
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    el.scrollTo({ top: el.scrollHeight, behavior: reduceMotion ? 'auto' : 'smooth' });
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
