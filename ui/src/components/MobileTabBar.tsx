import type { NavKey } from './Sidebar';

/** The compact bottom bar carries the global destinations and nothing else
 *  (docs/room-workbench.md, decision 3).
 *
 *  It used to carry Pipes and Files as tabs, which made a room-scoped tool
 *  look like a place you could stand without a room — and let the bar, the
 *  visible pane, and the panel's own tab strip disagree about where you were.
 *  Room tools are reached by entering a room; inside one, this bar gives way
 *  to the room's app bar (the behavior mockups/mobile-triptych.png shows). */
const TABS: { key: NavKey; label: string; glyph: string }[] = [
  { key: 'rooms', label: 'Rooms', glyph: '▦' },
  { key: 'fleet', label: 'Agent Fleet', glyph: '✦' },
  { key: 'settings', label: 'Settings', glyph: '⚙' },
];

export function MobileTabBar({ active, onNav }: { active: NavKey; onNav(key: NavKey): void }) {
  return (
    <nav className="mobile-tabbar" aria-label="Primary (mobile)">
      {TABS.map((tab) => {
        const on = active === tab.key;
        return (
          <button
            key={tab.key}
            type="button"
            className={`mtab${on ? ' active' : ''}`}
            aria-current={on ? 'page' : undefined}
            onClick={() => onNav(tab.key)}
          >
            <span className="mtab-glyph" aria-hidden="true">
              {tab.glyph}
            </span>
            <span className="mtab-label">{tab.label}</span>
          </button>
        );
      })}
    </nav>
  );
}
