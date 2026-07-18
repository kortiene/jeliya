import { describe, expect, it } from 'vitest';
import { documentTitle, pageRegion } from './landmarks';
import { en } from '../l10n/en';
import { BRAND } from '../l10n/tokens';
import type { Pane, PageRegion } from './landmarks';
import type { Shell } from './shell';

const PANES: Pane[] = ['rooms', 'room', 'inspector', 'fleet', 'settings'];
const SHELLS: Shell[] = ['compact', 'medium', 'wide'];

describe('pageRegion', () => {
  it('names exactly one page region for every pane x shell pair', () => {
    const valid: PageRegion[] = ['sidebar', 'center', 'inspector', 'fleet', 'settings'];
    for (const pane of PANES) {
      for (const shell of SHELLS) {
        expect(valid, `${pane}/${shell}`).toContain(pageRegion(pane, shell));
      }
    }
  });

  it('gives Fleet and Settings their own page on every shell', () => {
    for (const shell of SHELLS) {
      expect(pageRegion('fleet', shell)).toBe('fleet');
      expect(pageRegion('settings', shell)).toBe('settings');
    }
  });

  it('makes the room rail the page only on compact, where it is the whole screen', () => {
    expect(pageRegion('rooms', 'compact')).toBe('sidebar');
    // On medium and wide the rail is a complementary column beside the
    // workspace, which still renders (the "Choose a room" empty state).
    expect(pageRegion('rooms', 'medium')).toBe('center');
    expect(pageRegion('rooms', 'wide')).toBe('center');
  });

  it('makes the inspector the page only where it is the whole screen', () => {
    // Compact: the inspector IS the screen; the workspace is display:none.
    expect(pageRegion('inspector', 'compact')).toBe('inspector');
    // Medium and wide keep the workspace live beside it — on medium the
    // workspace reserves the drawer's width rather than running underneath it,
    // so it stays both usable and the page.
    expect(pageRegion('inspector', 'medium')).toBe('center');
    expect(pageRegion('inspector', 'wide')).toBe('center');
  });

  it('keeps the workspace as the page for an open room on every shell', () => {
    for (const shell of SHELLS) {
      expect(pageRegion('room', shell)).toBe('center');
    }
  });
});

describe('documentTitle', () => {
  const labels = {
    rooms: en.destRooms,
    fleet: en.destFleet,
    settings: en.destSettings,
    app: BRAND,
  };

  it('names each global destination', () => {
    expect(documentTitle('rooms', { labels })).toBe('Rooms · Jeliya');
    expect(documentTitle('fleet', { labels })).toBe('Agent Fleet · Jeliya');
    expect(documentTitle('settings', { labels })).toBe('Settings · Jeliya');
  });

  it('leads with the room inside a room', () => {
    expect(documentTitle('room', { roomName: 'Design System', labels })).toBe('Design System · Jeliya');
  });

  it('leads with the tool when the inspector is open', () => {
    expect(documentTitle('inspector', { roomName: 'Design System', destLabel: 'Files', labels })).toBe(
      'Files · Design System · Jeliya',
    );
  });

  it('falls back to the app alone rather than inventing a room name', () => {
    expect(documentTitle('room', { roomName: null, labels })).toBe('Jeliya');
    expect(documentTitle('room', { roomName: '   ', labels })).toBe('Jeliya');
  });

  it('passes room names through verbatim — they are user data, never copy', () => {
    expect(documentTitle('room', { roomName: '  Bug Triage  ', labels })).toBe('Bug Triage · Jeliya');
  });
});
