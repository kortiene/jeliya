/** Friendly error copy (issue #74; `docs/i18n.md` rule 3).
 *
 *  The daemon error code → designed-copy mapping every error surface uses,
 *  resolved AT RENDER TIME from the catalog the caller already holds, so a live
 *  language switch re-resolves error copy too. Mirrors Flutter's
 *  `app/lib/src/l10n/error_display.dart` code for code.
 *
 *  Why the daemon's own words are not the copy
 *  -------------------------------------------
 *  `docs/glossary-fr.md` decision 1: daemon and CLI output stays English.
 *  Operators and agents grep logs and search error text, so translating
 *  diagnostics is a support liability. The consequence for the UI is strict —
 *  the daemon's `message` and `hint` are TECHNICAL DETAIL, never the designed
 *  sentence. They belong in the collapsed "Technical details" disclosure, the
 *  Settings diagnostics card, and the diagnostics report; nowhere else.
 *
 *  This is exactly where `lib/errors.ts` went wrong. It covered nine codes, and
 *  its `default` branch returned `error.message` as the message and `error.hint`
 *  as the action — so under a French interface, any unmapped code (eight of the
 *  seventeen, including every share-staging and pipe error) rendered an English
 *  daemon string as the primary, designed copy of the card. The generic lead
 *  below is not a downgrade from that; it is the honest version, and it sits
 *  next to a disclosure that still shows the operator the exact raw error.
 *
 *  The obsolete `lib/errors.ts` was removed with the React migration so there
 *  is no untranslated mapper left for a future call site to import.
 *
 *  Codes cover the fourteen the daemon can put on the wire plus the three the
 *  client synthesizes (`connection_lost` from the transport, `file_too_large`
 *  and `file_unreadable` from share staging before any wire call), and unknown.
 */

import type { Catalog } from './catalog';

/** The catalog subset this file reads. */
type S = Catalog;

/** Plain-language title/message/action triple shared by every error surface. */
export interface FriendlyError {
  title: string;
  message: string;
  action?: string;
}

/** The minimum an error needs to be narrated. Structural, so a caller can pass
 *  a `DaemonErrorShape`, a `RequestError`, or anything else carrying a code —
 *  `lib/protocol.ts` types stay out of the l10n layer (rule 5: shared protocol
 *  modules compose no user-visible English, and the l10n layer returns the
 *  favor by not depending on them). */
export interface CodedError {
  code: string;
}

/** Map a daemon error to designed copy.
 *
 *  Unknown and future codes get the generic lead — never the daemon's English
 *  message or hint as primary copy. The raw `{code, message, hint}` stays
 *  available to the caller for the "Technical details" disclosure; this
 *  function deliberately does not read them, so there is no path by which they
 *  reach a title or a message. */
export function friendlyError(s: S, error: CodedError): FriendlyError {
  switch (error.code) {
    case 'peer_unreachable':
      return {
        title: s.errPeerUnreachableTitle,
        message: s.errPeerUnreachableMessage,
        action: s.errPeerUnreachableAction,
      };
    case 'bad_ticket':
      return {
        title: s.errBadTicketTitle,
        message: s.errBadTicketMessage,
        action: s.errBadTicketAction,
      };
    case 'ticket_expired':
      return {
        title: s.errTicketExpiredTitle,
        message: s.errTicketExpiredMessage,
        action: s.errTicketExpiredAction,
      };
    case 'room_not_open':
      return {
        title: s.errRoomNotOpenTitle,
        message: s.errRoomNotOpenMessage,
        action: s.errRoomNotOpenAction,
      };
    case 'not_a_member':
      return {
        title: s.errNotAMemberTitle,
        message: s.errNotAMemberMessage,
        action: s.errNotAMemberAction,
      };
    case 'room_unknown':
      return {
        title: s.errRoomUnknownTitle,
        message: s.errRoomUnknownMessage,
        action: s.errRoomUnknownAction,
      };
    case 'file_unauthorized':
      return {
        title: s.errFileUnauthorizedTitle,
        message: s.errFileUnauthorizedMessage,
        action: s.errFileUnauthorizedAction,
      };
    case 'hash_mismatch':
      return {
        title: s.errHashMismatchTitle,
        message: s.errHashMismatchMessage,
        action: s.errHashMismatchAction,
      };
    case 'connection_lost':
      return {
        title: s.errConnectionLostTitle,
        message: s.errConnectionLostMessage,
        action: s.errConnectionLostAction,
      };
    case 'invalid_params':
      return {
        title: s.errInvalidParamsTitle,
        message: s.errInvalidParamsMessage,
        action: s.errInvalidParamsAction,
      };
    case 'identity_missing':
      return {
        title: s.errIdentityMissingTitle,
        message: s.errIdentityMissingMessage,
        action: s.errIdentityMissingAction,
      };
    case 'identity_exists':
      return {
        title: s.errIdentityExistsTitle,
        message: s.errIdentityExistsMessage,
        action: s.errIdentityExistsAction,
      };
    case 'file_unavailable':
      return {
        title: s.errFileUnavailableTitle,
        message: s.errFileUnavailableMessage,
        action: s.errFileUnavailableAction,
      };
    case 'file_too_large':
      return {
        title: s.errFileTooLargeTitle,
        message: s.errFileTooLargeMessage,
        action: s.errFileTooLargeAction,
      };
    case 'file_unreadable':
      return {
        title: s.errFileUnreadableTitle,
        message: s.errFileUnreadableMessage,
        action: s.errFileUnreadableAction,
      };
    case 'pipe_denied':
      return {
        title: s.errPipeDeniedTitle,
        message: s.errPipeDeniedMessage,
        action: s.errPipeDeniedAction,
      };
    case 'internal':
      return {
        title: s.errInternalTitle,
        message: s.errInternalMessage,
        action: s.errInternalAction,
      };
    default:
      return {
        title: s.errUnknownTitle,
        message: s.errUnknownMessage,
        action: s.errUnknownAction,
      };
  }
}
