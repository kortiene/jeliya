import type { DaemonErrorShape } from './protocol';

export interface FriendlyError {
  title: string;
  message: string;
  action?: string;
}

export function friendlyError(error: DaemonErrorShape): FriendlyError {
  switch (error.code) {
    case 'peer_unreachable':
      return {
        title: "Couldn't reach the inviter",
        message: 'The invite is readable, but this device could not reach the room admin in time.',
        action: 'Ask the inviter to keep the room open, then retry. A fresh combined invite can help if the address changed.',
      };
    case 'bad_ticket':
      return {
        title: "This invite can't be used",
        message: 'The ticket is invalid for this identity, malformed, or no longer matches the room invite.',
        action: 'Ask for a new invite generated for your current identity id.',
      };
    case 'ticket_expired':
      return {
        title: 'This invite expired',
        message: 'The room rejected the ticket because its expiry time has passed.',
        action: 'Ask the inviter to generate a fresh ticket.',
      };
    case 'room_not_open':
      return {
        title: 'Open the room first',
        message: 'This action needs a live room session on your daemon.',
        action: 'Open the room, wait for it to sync, then try again.',
      };
    case 'not_a_member':
      return {
        title: "You're not an active member",
        message: 'The signed room history does not currently admit this identity as an active member.',
        action: 'Use a valid invite for this identity or ask the room owner to re-add you.',
      };
    case 'room_unknown':
      return {
        title: "This room isn't local yet",
        message: 'The daemon does not have enough room history to open this room.',
        action: 'Join with an invite, or open the room with a reachable peer hint.',
      };
    case 'connection_lost':
      return {
        title: 'Daemon connection lost',
        message: 'The local UI is not connected to jeliyad right now.',
        action: 'Wait for reconnect, then retry the action.',
      };
    default:
      return {
        title: 'Something went wrong',
        message: error.message,
        action: error.hint ?? undefined,
      };
  }
}
