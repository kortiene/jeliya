import type { RoomDest } from '../lib/routes';
import type { Catalog } from './catalog';

/** One render-time mapping from canonical room routes to catalog labels.
 *  Navigation, inspector landmarks, and document titles all use this seam so
 *  a destination cannot acquire three independently translated names. */
export function roomDestLabel(s: Catalog, dest: RoomDest): string {
  switch (dest) {
    case 'activity':
      return s.roomDestActivity;
    case 'people':
      return s.roomDestPeople;
    case 'agents':
      return s.roomDestAgents;
    case 'files':
      return s.roomDestFiles;
    case 'pipes':
      return s.roomDestPipes;
    default:
      return dest;
  }
}
