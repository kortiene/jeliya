import type { NavKey } from './Sidebar';
import { useStrings } from '../l10n/strings';
import { Glyph } from '../l10n/tokens';
import type { Catalog } from '../l10n/catalog';

/** The compact bottom bar carries the global destinations and nothing else
 *  (docs/room-workbench.md, decision 3).
 *
 *  It used to carry Pipes and Files as tabs, which made a room-scoped tool
 *  look like a place you could stand without a room — and let the bar, the
 *  visible pane, and the panel's own tab strip disagree about where you were.
 *  Room tools are reached by entering a room; inside one, this bar gives way
 *  to the room's app bar (the behavior mockups/mobile-triptych.png shows).
 *
 *  The labels come from the SAME catalog keys the rail uses (`Sidebar.tsx`),
 *  not a second copy of the words. Decision 1 exists to stop a rail entry and a
 *  tab naming one destination differently — two literal arrays were how that
 *  would have happened first. The glyphs are decoration and live in `tokens.ts`,
 *  outside the catalog, because a translator has nothing to do with `▦`. */
const TABS: { key: NavKey; messageKey: keyof Catalog; glyph: string }[] = [
  { key: 'rooms', messageKey: 'destRooms', glyph: Glyph.rooms },
  { key: 'fleet', messageKey: 'destFleet', glyph: Glyph.fleet },
  { key: 'settings', messageKey: 'destSettings', glyph: Glyph.settings },
];

export function MobileTabBar({ active, onNav }: { active: NavKey; onNav(key: NavKey): void }) {
  const s = useStrings();
  return (
    <nav className="mobile-tabbar" aria-label={s.shellNavPrimaryMobile}>
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
            <span className="mtab-label">{s[tab.messageKey] as string}</span>
          </button>
        );
      })}
    </nav>
  );
}
