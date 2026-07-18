import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ROOM_DESTS } from '../lib/routes';
import type { RoomDest } from '../lib/routes';
import { roomDestLabel } from '../l10n/destinations';
import { useFormats, useStrings } from '../l10n/strings';
import { Punct } from '../l10n/tokens';

function tabCountLabel(count: number, formatted: string): string {
  return count > 99 ? Punct.countCap : formatted;
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
  controlsId,
}: {
  dest: RoomDest;
  counts: Partial<Record<RoomDest, number>>;
  onDest(dest: RoomDest): void;
  /** The id of the tabpanel these tabs control, or undefined when no panel is
   *  rendered — which is exactly the Activity destination, where the room's
   *  workspace IS the content and the inspector is closed.
   *
   *  This has to be conditional: `aria-controls` pointed unconditionally at
   *  `panel-body`, so on Activity it named an element that does not exist. That
   *  is an `aria-valid-attr-value` failure (critical), and a dangling idref is
   *  worse for assistive tech than an absent optional attribute (issue #72). */
  controlsId?: string;
}) {
  const s = useStrings();
  const formats = useFormats();
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
    <div className="panel-tabs room-nav" role="tablist" aria-label={s.roomNavLabel} onKeyDown={onKeyDown}>
      {ROOM_DESTS.map((d) => {
        const count = counts[d];
        return (
          <button
            key={d}
            type="button"
            role="tab"
            id={`room-tab-${d}`}
            aria-selected={dest === d}
            aria-controls={controlsId}
            tabIndex={dest === d ? 0 : -1}
            className={dest === d ? 'active' : ''}
            onClick={() => onDest(d)}
          >
            <span className="panel-tab-label">{roomDestLabel(s, d)}</span>
            {count !== undefined && count > 0 ? (
              <span className="count">{tabCountLabel(count, formats.count(count))}</span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
