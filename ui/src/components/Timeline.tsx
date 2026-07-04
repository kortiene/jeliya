import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import type { FileEntry, TimelineEvent } from '../lib/protocol';
import { dayLabel, extOf, fileTint, formatBytes, formatTime, labelTone, prettyLabel, shortId } from '../lib/format';
import { Avatar, FetchControl, FetchDetail, ProgressBar, SenderName } from './ui';
import type { FetchState } from './ui';

function FileTile({
  file,
  state,
  onFetch,
}: {
  file: { file_id: string; name: string; size: number; mime: string };
  state?: FetchState;
  onFetch(fileId: string): void;
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
        <FetchControl state={state} onFetch={() => onFetch(file.file_id)} />
      </div>
      <FetchDetail state={state} />
    </div>
  );
}

function EventCard({
  event,
  files,
  fetches,
  onFetch,
  onShowPipes,
}: {
  event: TimelineEvent;
  files: FileEntry[];
  fetches: Record<string, FetchState>;
  onFetch(fileId: string): void;
  onShowPipes(): void;
}) {
  const senderId = event.sender.identity_id;
  const time = formatTime(event.ts);

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

    case 'message': {
      // The desktop mockup renders every message uniformly left-aligned with an
      // avatar — not a two-sided chat — so own messages are not special-cased.
      return (
        <div className="msg-row">
          <Avatar id={senderId} />
          <div className="msg-col">
            <div className="msg-meta">
              <SenderName id={senderId} className="msg-sender" />
              {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
            </div>
            <div className="msg-bubble">{event.body}</div>
          </div>
        </div>
      );
    }

    case 'agent_status': {
      const label = event.label ?? 'status';
      const artifacts = event.artifacts ?? [];
      return (
        <div className="event-card">
          <Avatar id={senderId} />
          <div className="event-main">
            <div className="event-head">
              <SenderName id={senderId} className="event-sender" />
              {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
              <span className={`chip chip-label tone-${labelTone(label)}`}>{prettyLabel(label)}</span>
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
      return (
        <div className="event-card">
          <Avatar id={senderId} />
          <div className="event-main">
            <div className="event-head">
              <SenderName id={senderId} className="event-sender" />
              {event.sender.role === 'agent' ? <span className="chip chip-role">AGENT</span> : null}
              <span className="muted">shared a file</span>
              <time dateTime={new Date(event.ts).toISOString()}>{time}</time>
            </div>
            <FileTile file={event.file} state={fetches[event.file.file_id]} onFetch={onFetch} />
          </div>
        </div>
      );
    }

    case 'pipe_opened': {
      if (!event.pipe) return null;
      return (
        <div className="event-card">
          <Avatar id={senderId} />
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
                <strong className="mono">{event.pipe.target}</strong>
                <span className="muted">
                  authorized peer: <SenderName id={event.pipe.authorized_peer} />
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

export function Timeline({
  events,
  files,
  fetches,
  loading,
  onFetch,
  onShowPipes,
}: {
  events: TimelineEvent[];
  files: FileEntry[];
  fetches: Record<string, FetchState>;
  loading: boolean;
  onFetch(fileId: string): void;
  onShowPipes(): void;
}) {
  const scroller = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);

  useEffect(() => {
    const el = scroller.current;
    if (el && stickToBottom.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [events.length]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 140;
  };

  let lastDay = '';
  const rows: { key: string; node: ReactNode }[] = [];
  for (const event of events) {
    const day = dayLabel(event.ts);
    if (day !== lastDay) {
      lastDay = day;
      rows.push({
        key: `day-${day}-${event.event_id}`,
        node: (
          <div className="day-divider">
            <span>{day}</span>
          </div>
        ),
      });
    }
    rows.push({
      key: event.event_id,
      node: (
        <EventCard event={event} files={files} fetches={fetches} onFetch={onFetch} onShowPipes={onShowPipes} />
      ),
    });
  }

  return (
    <div className="timeline" ref={scroller} onScroll={onScroll} role="log" aria-label="Room timeline" aria-busy={loading}>
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
        <div key={row.key} className="timeline-row">
          {row.node}
        </div>
      ))}
      {!loading && events.length === 0 ? (
        <div className="timeline-empty muted">No events yet — say something below.</div>
      ) : null}
    </div>
  );
}
