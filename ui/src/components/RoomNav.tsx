import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ROOM_DESTS } from '../lib/routes';
import type { RoomDest } from '../lib/routes';

function tabCountLabel(count: number): string {
  return count > 99 ? '99+' : String(count);
}

/** The room's nested navigation — one model, every shell
 *  (docs/room-workbench.md, decisions 1 and 3).
 *
 *  Activity is a tab like the others because it is a destination like the
 *  others: it is the room with no tool open. That is what lets "close the
 *  inspector" and "go to Activity" be the same navigation instead of two
 *  mechanisms that can disagree.
 *
 *  Whoever the room's tool surface is, carries this strip. On wide the tool
 *  opens *beside* the workspace, so the workspace keeps it. On medium and
 *  compact the tool covers the workspace — as a drawer or as the whole pane —
 *  so the tool carries it instead, and the workspace's copy is hidden rather
 *  than left buried under the drawer where it would still be in the
 *  accessibility tree, still report aria-selected, and still swallow clicks
 *  into whatever floats on top of it.
 */
export function RoomNav({
  dest,
  counts,
  onDest,
}: {
  dest: RoomDest;
  counts: Partial<Record<RoomDest, number>>;
  onDest(dest: RoomDest): void;
}) {
  const labels: Record<RoomDest, string> = {
    activity: 'Activity',
    people: 'People',
    agents: 'Agents & Runs',
    files: 'Files',
    pipes: 'Pipes',
  };

  // Full ARIA tabs keyboard pattern: arrows move between tabs (one tab stop via
  // roving tabindex), Home/End jump to the ends. Without this the tab roles
  // would announce a pattern that does not actually work.
  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const idx = ROOM_DESTS.indexOf(dest);
    let next = idx;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (idx + 1) % ROOM_DESTS.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = (idx - 1 + ROOM_DESTS.length) % ROOM_DESTS.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = ROOM_DESTS.length - 1;
    else return;
    e.preventDefault();
    onDest(ROOM_DESTS[next]);
    document.getElementById(`room-tab-${ROOM_DESTS[next]}`)?.focus();
  };

  return (
    <div className="panel-tabs room-nav" role="tablist" aria-label="Room tools" onKeyDown={onKeyDown}>
      {ROOM_DESTS.map((d) => {
        const count = counts[d];
        return (
          <button
            key={d}
            type="button"
            role="tab"
            id={`room-tab-${d}`}
            aria-selected={dest === d}
            aria-controls="panel-body"
            tabIndex={dest === d ? 0 : -1}
            className={dest === d ? 'active' : ''}
            onClick={() => onDest(d)}
          >
            <span className="panel-tab-label">{labels[d]}</span>
            {count !== undefined && count > 0 ? <span className="count">{tabCountLabel(count)}</span> : null}
          </button>
        );
      })}
    </div>
  );
}
