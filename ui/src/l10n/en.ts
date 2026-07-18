/** English — the source of truth (issue #74).
 *
 *  Every other locale is typed as `Catalog`, so this file's shape is the
 *  contract: add a key here and `fr.ts` stops compiling until it is translated.
 *
 *  Where a string already exists in the Flutter catalog
 *  (`app/lib/src/l10n/arb/app_en.arb`) its wording is REUSED verbatim rather
 *  than re-written. The two clients ship the same product; a rail that says
 *  "Left & removed" in one and "Departed" in the other is a translation bug in
 *  both languages at once.
 *
 *  Casing is sentence case, matching the Flutter catalog's normalization pass.
 *  All-caps treatments are `toUpperCase()` at render time (rule 7), never here.
 */

import type { LocaleCatalog } from './catalog';

export const en: LocaleCatalog = {
  // -- wire enums and daemon errors ---------------------------------------------
  //
  // Inlined rather than spread from another module ON PURPOSE: the CI gate
  // (scripts/check-ui-i18n.mjs) reads these files with a restricted scanner, and
  // a spread it cannot follow makes the parity, emptiness and typography rules
  // silently stop running — a gate that reports nothing looks identical to a gate
  // that finds nothing. One locale, one file, every value visible.
  wireRoleOwnerInline: 'owner',
  wireRoleMemberInline: 'member',
  wireRoleAgentInline: 'agent',

  panelRoleOwner: 'Owner',
  panelRoleAgent: 'Agent',
  panelRoleMember: 'Member',

  memberStatusMember: 'Member',
  wireStatusInvited: 'Invited',
  wireStatusLeft: 'Left',
  wireStatusRemoved: 'Removed',
  memberStatusUnknown: 'Unknown',

  wirePathDirect: 'direct',
  wirePathRelay: 'relay',

  wireModeLoopback: 'loopback',
  wireModeReal: 'real',

  wireConnConnectedInline: 'connected',
  wireConnConnectingInline: 'connecting',
  wireConnReconnectingInline: 'reconnecting',
  wireConnDisconnectedInline: 'disconnected',

  errPeerUnreachableTitle: "Couldn't reach the inviter",
  errPeerUnreachableMessage:
    'The invite is readable, but this device could not reach the room admin in time.',
  errPeerUnreachableAction:
    'Ask the inviter to keep the room open, then retry. A fresh combined invite can help if the address changed.',

  errBadTicketTitle: "This invite can't be used",
  errBadTicketMessage:
    'The ticket is invalid for this identity, malformed, or no longer matches the room invite.',
  errBadTicketAction: 'Ask for a new invite generated for your current identity ID.',

  errTicketExpiredTitle: 'This invite expired',
  errTicketExpiredMessage: 'The room rejected the ticket because its expiry time has passed.',
  errTicketExpiredAction: 'Ask the inviter to generate a fresh ticket.',

  errRoomNotOpenTitle: 'Open the room first',
  errRoomNotOpenMessage: 'This action needs a live room session on your daemon.',
  errRoomNotOpenAction: 'Open the room, wait for it to sync, then try again.',

  errNotAMemberTitle: "You're not an active member",
  errNotAMemberMessage:
    'The signed room history does not currently admit this identity as an active member.',
  errNotAMemberAction: 'Use a valid invite for this identity or ask the room owner to re-add you.',

  errRoomUnknownTitle: "This room isn't local yet",
  errRoomUnknownMessage: 'The daemon does not have enough room history to open this room.',
  errRoomUnknownAction: 'Join with an invite, or open the room with a reachable peer hint.',

  errFileUnauthorizedTitle: 'Not authorized for this file',
  errFileUnauthorizedMessage:
    'Every reachable provider refused the transfer because the signed history does not admit this identity for it.',
  errFileUnauthorizedAction: 'Ask the sender to re-share the file or re-invite you, then retry.',

  errHashMismatchTitle: 'Security check failed',
  errHashMismatchMessage:
    'The fetched bytes did not match the file hash. This is a hard stop — the copy is discarded, never shown.',
  errHashMismatchAction: 'Ask the sender to re-share the file. Do not retry the same copy.',

  errConnectionLostTitle: 'Daemon connection lost',
  errConnectionLostMessage: 'The local UI is not connected to jeliyad right now.',
  errConnectionLostAction: 'Wait for reconnect, then retry the action.',

  errInvalidParamsTitle: "This request wasn't valid",
  errInvalidParamsMessage: 'The daemon rejected one of the values in this request.',
  errInvalidParamsAction: 'Check what you entered, then try again.',

  errIdentityMissingTitle: 'No identity on this daemon yet',
  errIdentityMissingMessage: 'This action needs your identity, and one has not been created here.',
  errIdentityMissingAction: 'Create your identity first, then retry.',

  errIdentityExistsTitle: 'An identity already exists',
  errIdentityExistsMessage: 'This daemon already holds an identity — a second one cannot be created.',
  errIdentityExistsAction: 'Use the existing identity shown in Settings.',

  errFileUnavailableTitle: 'File not available right now',
  errFileUnavailableMessage: 'No provider is online for this file yet.',
  errFileUnavailableAction: 'Recheck when the sender is back online.',

  errFileTooLargeTitle: 'This file is too large to share',
  errFileTooLargeMessage: 'Shares are capped at 100 MiB per file.',
  errFileTooLargeAction: 'Pick a smaller file, or split the content.',

  errFileUnreadableTitle: "This file couldn't be read",
  errFileUnreadableMessage: 'The picked file could not be opened from disk.',
  errFileUnreadableAction: 'Check the file still exists and is readable, then retry.',

  errPipeDeniedTitle: 'Pipe access denied',
  errPipeDeniedMessage: 'This pipe does not authorize your identity.',
  errPipeDeniedAction: 'Ask the pipe owner to expose it to your identity.',

  errInternalTitle: 'The daemon hit an unexpected failure',
  errInternalMessage: 'This request failed for a reason the daemon could not classify.',
  errInternalAction: 'Retry; if it keeps failing, copy diagnostics from Settings and report it.',

  errUnknownTitle: 'Something went wrong',
  errUnknownMessage: 'The daemon reported an error this app has no specific copy for.',
  errUnknownAction: 'Open Technical details for the exact error, then retry.',

  localeTag: 'en',

  // -- common ------------------------------------------------------------------
  commonRetry: 'Retry',
  commonCancel: 'Cancel',
  commonClose: 'Close',
  commonClear: 'Clear',
  commonSave: 'Save',
  commonBack: 'Back',
  commonCopy: 'Copy',
  commonCopied: 'Copied ✓',
  commonCopyFailed: "Couldn’t copy — select the text and copy it manually.",
  commonReconnecting: 'Reconnecting…',
  commonUnknown: 'Unknown',
  commonOptional: '(optional)',
  commonOptionalFieldLabel: '{label} {optional}',
  commonServing: 'Serving',
  commonServingTooltip: 'This daemon is already serving this file to peers.',
  commonFileExtFallback: 'file',
  commonTechnicalDetails: 'Technical details',
  commonTaskProgress: 'Task progress',
  commonChecking: 'Checking…',
  commonFetch: 'Fetch',
  commonFetching: 'Fetching…',
  commonVerified: '✓ Verified',
  commonFetched: '✓ Fetched',
  commonFailed: '✕ Failed',
  commonRecheck: 'Recheck',
  commonOpenFile: 'Open file',
  commonCopyPath: 'Copy path',
  commonCopySavedFilePath: 'Copy saved file path',
  commonNoProviderOnline: 'No provider online',
  commonSetLocalNameFor: (id) => `${id}\nClick to set a local name`,

  // -- file fetching -----------------------------------------------------------
  fetchProvidersListedOnline: (n, formatted = String(n)) =>
    `${formatted} ${n === 1 ? 'provider' : 'providers'} listed; at least one is online`,
  fetchProvidersListedOffline: (n, formatted = String(n)) =>
    `${formatted} ${n === 1 ? 'provider' : 'providers'} listed; none are online right now`,
  fetchRecheckProvidersFor: (file) => `Recheck providers for ${file}`,
  fetchFileNamed: (file) => `Fetch ${file}`,
  fetchOpenFileNamed: (file) => `Open file ${file}`,
  fetchCopySavedPathFor: (file) => `Copy saved path for ${file}`,
  fetchRetryNamed: (file) => `Retry fetching ${file}`,
  fetchVerifiedTooltip: (path) => `verified · ${path}`,
  fetchFetchedTooltip: (path) => `fetched · ${path}`,
  fetchDetailVerified: 'Verified · {bytes} · saved to {path}',
  fetchDetailFetched: 'Fetched · {bytes} · saved to {path}',
  fetchOpenLocalFileCopy: 'Open local file copy',
  fetchErrFileUnavailable: 'No provider is online for this file yet. Recheck when the sender is back online.',
  fetchErrFileUnauthorized:
    'Every provider refused this fetch — your identity is not authorized for it. Ask the sender to re-share or re-invite you.',
  fetchErrHashMismatch:
    "This file failed a security check and wasn’t saved — it may have been corrupted or tampered with in transit.",

  // -- boot --------------------------------------------------------------------
  bootSyncing: 'Syncing…',
  bootNotConnected: 'Not connected.',
  bootContacting: 'Contacting daemon…',
  bootRetryingHint: 'Retrying with backoff — start {daemon} or pass {port}.',

  // -- shell / connection ------------------------------------------------------
  shellConnectionLost: (transport) => `Connection to daemon lost — reconnecting… (${transport})`,
  shellDisconnected: 'Disconnected from daemon.',
  shellSkipToMain: 'Skip to main content',
  shellSkipToComposer: 'Skip to message composer',
  shellConnConnected: 'Connected',
  shellConnConnecting: 'Connecting…',
  shellConnReconnecting: 'Reconnecting…',
  shellConnDisconnected: 'Disconnected',
  shellNavPrimary: 'Primary',
  shellNavPrimaryMobile: 'Primary (mobile)',

  // -- global destinations -----------------------------------------------------
  destRooms: 'Rooms',
  destFleet: 'Agent Fleet',
  destSettings: 'Settings',

  // -- room destinations -------------------------------------------------------
  roomDestActivity: 'Activity',
  roomDestPeople: 'People',
  roomDestAgents: 'Agents & Runs',
  roomDestFiles: 'Files',
  roomDestPipes: 'Pipes',

  // -- rooms list --------------------------------------------------------------
  roomsYourRooms: 'Your Rooms',
  roomsChoose: 'Choose a room.',
  roomsCreate: 'Create room',
  roomsJoinWithTicket: 'Join with a ticket',
  roomsSearchPlaceholder: 'Search rooms…',
  roomsSearchLabel: 'Search rooms by name or short id',
  roomsFilterLegend: 'Filter rooms by lifecycle',
  roomsFilterAll: 'All',
  roomsFilterActive: 'Active',
  roomsFilterDeparted: 'Left & removed',
  roomsSectionPinned: 'Pinned',
  roomsSectionArchived: 'Archived',
  roomsSectionCount: (n, formatted = String(n)) => `(${formatted})`,
  roomsEmpty: 'No rooms yet',
  roomsNoMatch: (query) => `No rooms match “${query}”.`,
  roomsNoneInFilter: 'No rooms in this filter.',
  roomsUnread: 'Unread',
  roomsMemberCount: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} member` : `${formatted} members`,
  roomsUntitled: 'Untitled room',
  roomsStateOpen: 'Open',
  roomsStateClosed: 'Closed',
  roomsStateLeft: 'Left',
  roomsStateRemoved: 'Removed',
  roomsSessionOpen: 'Session open',
  roomsYouLeft: 'You left this room',
  roomsYouWereRemoved: 'You were removed from this room',
  roomsPin: (room) => `Pin ${room}`,
  roomsUnpin: (room) => `Unpin ${room}`,
  roomsArchive: (room) => `Archive ${room}`,
  roomsRestore: (room) => `Restore ${room}`,
  roomsPinShort: 'Pin',
  roomsUnpinShort: 'Unpin',
  roomsArchiveShort: 'Archive',
  roomsRestoreShort: 'Restore from archive',
  roomsRailLabel: 'Room rail',
  roomsListLabel: 'Rooms',
  roomsProfile: 'Profile & settings',
  roomsProfileHandle: (id) => `@${id}`,

  // -- room recovery surfaces --------------------------------------------------
  roomNotOnDevice: 'That room isn’t on this device',
  roomNotOnDeviceDetail:
    'Nothing here matches {id}. It may live on another device, or you may not have joined it yet.',
  roomBackToRooms: 'Back to Rooms',
  roomLeftDetail: 'Your departure is published to the room’s signed log. You’ll need a new invite to rejoin.',
  roomRemovedDetail: 'Your removal is published to the room’s signed log. You’ll need a new invite to rejoin.',

  // -- identity ----------------------------------------------------------------
  identitySelf: 'You',
  identityP2P: 'P2P Identity',
  identityCopy: 'Copy identity ID',
  identityEndpointShort: (id) => `ep ${id}`,
  identityEndpointTitle: (id) => `endpoint ${id}`,

  // -- device-local self label -------------------------------------------------
  selfLabelTitle: 'Your name on this device',
  selfLabelHint: 'Only visible to you — never shared or signed.',
  selfLabelPlaceholder: 'e.g. Alex',

  // -- settings ----------------------------------------------------------------
  settingsTitle: 'Settings',
  settingsLanguageLabel: 'Language',
  settingsFormattingLabel: 'Dates & numbers',
  settingsLocaleSystemDefault: 'System default',
  settingsIdentityLabel: 'P2P Identity',
  settingsSelfLabelNote:
    'Your name is a local label — it never changes your cryptographic identity, which is unrecoverable if this device or its data folder is lost.',
  settingsEndpointLabel: 'Endpoint',
  settingsDaemonLabel: 'Daemon',
  settingsSupportLabel: 'Support',
  settingsDiagnosticsTitle: 'Diagnostics',
  settingsDiagnosticsCopy:
    'Copy a privacy-safe snapshot for bug reports: daemon version, connection state, room counts, peer state, file-transfer state, pipe state, and the latest UI error.',
  settingsNoMessageBodies: 'No message bodies',
  settingsNoInviteTickets: 'No invite tickets',
  settingsNoFileNamesOrPaths: 'No file names or full local paths',
  settingsNoFullIdentityIds: 'No full identity IDs',
  settingsLastCapturedError: 'Last captured error',
  settingsNoErrorCaptured: 'No UI action error captured in this session.',
  settingsCopyDiagnostics: 'Copy diagnostics',
  settingsCopiedDiagnostics: 'Copied diagnostics',
  settingsReportIssue: 'Report issue',
  settingsIssueReportTitle: 'Jeliya issue report',

  // -- fleet ------------------------------------------------------------------
  fleetLivenessWorking: 'Working',
  fleetLivenessOnline: 'Online',
  fleetLivenessStale: 'Stale',
  fleetLivenessOffline: 'Offline',
  fleetSparkLoading: 'Loading status history',
  fleetSparkEmpty: 'No status history yet',
  fleetSparkEvents: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} status event` : `${formatted} status events`,
  fleetSparkNumericProgress: (n, formatted = String(n)) => `${formatted} with numeric progress`,
  fleetSparkNoNumericProgress: 'no numeric progress',
  fleetCopyIdentityIdFor: (name) => `Copy identity ID for ${name}`,
  fleetLastStatusHint: 'Last posted status — its liveness no longer supports it',
  fleetLastStatus: (label) => `Last: ${label}`,
  fleetNoStatusPosted: 'No status posted yet.',
  fleetLastUpdate: (relativeTime) => `Last update ${relativeTime}`,
  fleetNeverSeen: 'Never seen',
  fleetOpenRoom: 'Open room',
  fleetCoverageEmpty: 'Room coverage: no rooms yet.',
  fleetCoverage: (covered, total, percent) =>
    `Rooms with an agent: ${covered} of ${total} (${percent}).`,
  fleetAttentionFailed: 'Failed',
  fleetAttentionReview: 'Awaiting review',
  fleetAttentionStale: 'Stale',
  fleetAttentionOffline: 'Offline after work',
  fleetNeedsAttention: 'Needs attention',
  fleetNeedsAttentionEmpty: 'Nothing needs attention right now.',
  fleetFilterAll: 'All',
  fleetFilterLive: 'Live',
  fleetFilterNeedsAttention: 'Needs attention',
  fleetFilterWorking: 'Working',
  fleetFilterOffline: 'Offline',
  fleetSearchPlaceholder: 'Search agents…',
  fleetSearchAgents: 'Search agents',
  fleetAddAgent: '＋ Add agent',
  fleetFilterAgents: 'Filter agents',
  fleetLoadingAgents: 'Loading agents',
  fleetEmptyNoAgents: 'No agents in any room yet. Use “Add agent” to mint an invite.',
  fleetEmptyNoMatch: 'No agents match this filter.',

  // -- add agent --------------------------------------------------------------
  addAgentTitle: 'Add an agent',
  addAgentNoOwnedRooms:
    'You don’t own any rooms yet. Create a room first — agent invites can only be minted for a room you own.',
  addAgentIntro:
    'Mint an agent-role ticket for a room you own. This {emphasis} — running the command below on the agent’s machine is a deliberate, human step (the security boundary).',
  addAgentIntroEmphasis: 'does not start anything',
  addAgentRoomLabel: 'Room',
  addAgentIdentityLabel: 'Agent identity ID',
  addAgentIdentityPlaceholder: '64-hex identity ID (from jeliya-agent.mjs --identity-only)',
  addAgentWorkerLabel: 'Worker',
  addAgentWorkerEchoOption: 'echo (safe — no real execution, for trying the flow)',
  addAgentWorkerClaudeOption:
    'claude (runs real commands — arbitrary code/file execution for this room’s allowlisted senders)',
  addAgentClaudeWarning:
    'WARNING — --worker claude runs the claude CLI with --permission-mode acceptEdits on every triggered message from an allowlisted sender. That is arbitrary code / file execution on this host. Only enable it for a room and senders you trust.',
  addAgentMintInvite: 'Mint agent invite',
  addAgentMinting: 'Minting…',
  addAgentResultIntro:
    'Run this on the agent’s machine to bring it into the room. The daemon has no “spawn agent” call — this is copied and run by a human on purpose.',
  addAgentLaunchCommandLabel: 'Agent launch command',
  addAgentCopyCommand: 'Copy command',
  addAgentGuidance:
    'The runner lives in the repo — clone it and run this from the checkout (no {npm} needed; Node 22+ required). Installed {jeliyad} via brew/script instead of building? Prefix the command with {prefix} so the runner finds it. Full guide: {guide}.',
  addAgentTicketOnly: 'Ticket only (if you assemble the command yourself):',
  addAgentCopyTicket: 'Copy ticket',
  addAgentNoDialableAddr:
    'This daemon reported no dialable address — the agent may connect via relay or discovery.',
  addAgentNewInvite: 'New invite',

  // -- invite -----------------------------------------------------------------
  inviteExpiry1h: '1 hour',
  inviteExpiry24h: '24 hours',
  inviteExpiry7d: '7 days',
  inviteExpiryNever: 'No expiry',
  inviteLifecycleJoined: 'Joined',
  inviteLifecycleExpired: 'Expired',
  inviteLifecycleWaiting: 'Waiting',
  inviteLifecycleJoinedCopy:
    'They have joined the room — the roster confirms an active membership.',
  inviteLifecycleExpiredCopy:
    'This ticket has expired before they joined. Send a fresh one below.',
  inviteLifecycleWaitingCopy:
    'Waiting for them to join. This updates on its own when the roster changes.',
  inviteExpiryErrorTitle: "This expiry isn't valid",
  inviteExpiryErrorMessage: 'Expiry must be a positive number of seconds.',
  inviteExpiryErrorHint: 'Leave it blank or use a value like 3600.',
  inviteShareTitle: 'Jeliya room invite',
  inviteTitle: 'Invite to room',
  inviteReadyToSend: 'Ready to send.',
  inviteReadyToSendCopy:
    'Stay in this room until they join. If they still see “Couldn’t reach the inviter,” copy a fresh invite and retry.',
  inviteNoDialableAddress: 'No dialable address reported yet.',
  inviteNoDialableAddressCopy:
    'Keep this room open. The joiner may still connect via discovery or relay, but a fresh room address is more reliable.',
  inviteCombinedCopy:
    'Send this one paste to the invitee — it is the ticket and your dialable address together. They paste it into “Join with a ticket” and the address fills in automatically.',
  inviteTicketOnlyCopy: 'Send this ticket to the invitee. They join with it (room.join).',
  inviteCombinedInviteLabel: 'Combined invite (ticket and peer address)',
  inviteInviteTicketLabel: 'Invite ticket',
  inviteCopyInvite: 'Copy invite',
  inviteCopyTicket: 'Copy ticket',
  inviteShareInvite: 'Share invite',
  inviteShareTicket: 'Share ticket',
  inviteQrLabel: 'QR code for the room invite — scan on another device to join',
  inviteQrCombinedCaption: 'Scan to join — this is the same invite as above.',
  inviteQrTicketCaption: 'Scan to import this ticket on another device.',
  inviteSeparatelySummary: 'Send the ticket and address separately',
  inviteCopyAddress: 'Copy address',
  inviteNoDialableAddressNote:
    'This daemon has not reported a dialable address — the joiner may connect via relay or discovery.',
  inviteGenerating: 'Generating…',
  inviteAgain: 'Invite again',
  inviteNewInvite: 'New invite',
  inviteAlreadyInvited:
    'You have already invited this identity and they have not joined yet. Send a fresh invite below.',
  inviteIntro:
    'Tickets are bound to one identity. Ask the invitee for their identity ID — it is shown on their onboarding screen and in their sidebar footer, with a copy button.',
  inviteRoomOpenForInviting: 'This room is open for inviting.',
  inviteRoomOpenForInvitingCopy:
    'Keep it open until the invitee finishes joining. Jeliya can only bootstrap them while an owner is reachable.',
  inviteInviteeIdentityId: 'Invitee identity ID',
  inviteInviteePlaceholder: '64-hex identity ID',
  inviteIdentityInvalid:
    'That is not a valid identity ID — it must be exactly 64 hexadecimal characters.',
  inviteIdentityHint:
    'Paste the invitee’s 64-hex identity ID, shown on their onboarding screen and sidebar footer.',
  inviteRoleLabel: 'Role',
  inviteRoleMemberConsequence:
    '{role} — a person in the room: reads and posts, shares files. No command execution.',
  inviteRoleAgentConsequence:
    '{role} — an automated participant that can act on this room’s allowlisted messages.',
  inviteAgentWarning:
    'WARNING — an agent invite authorizes an automated participant. Minting the ticket does not start anything: a human must run the agent on its own machine, where it can execute this room’s allowlisted commands — arbitrary code / file execution on that host. Only invite an agent for a room and senders you trust.',
  inviteTicketExpiryLabel: 'Ticket expiry',
  inviteAdvancedExpiry: 'Advanced / custom expiry',
  inviteCustomExpiryLabel: 'Custom expiry seconds',
  inviteCustomExpiryOverride: '(overrides the preset above)',
  inviteSendFresh: 'Send a fresh invite',
  inviteGenerateTicket: 'Generate ticket',

  // -- room header and inspector ---------------------------------------------
  roomNavLabel: 'Room tools',
  roomBackToActivity: 'Back to Activity',
  roomCloseInspector: 'Close inspector',
  roomInformation: 'Room information',
  roomInfoRoom: 'Room',
  roomInfoSession: 'Session',
  roomInfoAgents: 'Agents',
  roomInfoInvites: 'Invites',
  roomLoadingMembers: 'Loading members…',
  commonMemberCount: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} member` : `${formatted} members`,
  roomHeaderAgentCount: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} agent` : `${formatted} agents`,
  roomHeaderInvitesPending: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} invite pending` : `${formatted} invites pending`,
  roomHeaderNoPeersConnected: 'No peers connected',
  roomHeaderPeerToPeer: 'Peer-to-Peer',
  roomHeaderRelayOnly: 'Relay only',
  roomHeaderPeerConnected: 'Connected',
  roomHeaderPeerConnecting: 'Connecting…',
  roomHeaderShareFile: 'Share file',
  roomHeaderOpenPipe: 'Open pipe',
  roomHeaderInvite: 'Invite',
  roomHeaderPeerConnections: 'Peer connections',
  roomHeaderPeerStateConnected: 'connected',
  roomHeaderPeerStateConnecting: 'connecting',
  roomHeaderPeerStateOffline: 'offline',

  // -- People, Agents, Files, and Pipes inspector -----------------------------
  panelMembersEmpty: 'No members have synced for this room yet.',
  panelRoomMemberCount: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} room member` : `${formatted} room members`,
  panelRosterCopy:
    'Roster from the signed room history. Statuses reflect membership events, not live peer reachability.',
  panelRoomRoster: 'Room roster',
  panelInvitedCount: (n, formatted = String(n)) => `${formatted} invited`,
  panelThisDevice: 'this device',
  panelLeave: 'Leave',
  panelOwnerStays: 'Owner stays',
  panelOwnerStaysTitle: 'Owners cannot leave until ownership transfer exists.',
  panelAgentsEmpty: 'No agent members in this room yet. Invite one with role “agent”.',
  panelNoStatusPostedYet: 'No status posted yet',
  panelAgentStatusFooter: (status) => `status: ${status}`,
  panelKindBinary: 'binary',
  panelKindText: 'text',
  panelKindFile: 'file',
  panelFilesHeroEmptyDetail: 'Share a readable path and peers can fetch a verified copy over P2P.',
  panelFilesHeroDetail: (totalBytes, availableCount, formatted = String(availableCount)) =>
    `${totalBytes} in the room · ${formatted} fetchable here`,
  panelNFetched: (n, formatted = String(n)) => `${formatted} fetched`,
  panelServedByYou: (n, formatted = String(n)) => `${formatted} served by you`,
  panelNoSharedFilesYet: 'No shared files yet',
  panelSharedFileCount: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} shared file` : `${formatted} shared files`,
  panelFileAvailabilityLabel: 'File availability',
  panelFetchableNow: 'Fetchable now',
  panelFetchableNowValue: (available, total, availableText = String(available), totalText = String(total)) =>
    `${availableText}/${totalText}`,
  panelProviderDevices: 'Provider devices',
  panelFilesShareToggle: 'Share a file',
  panelFilesShareToggleClose: 'Close',
  panelShareCardTitle: 'Choose a file to share',
  panelShareCardHelp:
    'Pick a local file. Jeliya uploads it to this daemon, imports it into the room blob store, and verifies it by content hash.',
  panelHashCheckedBadge: 'hash checked',
  panelHashCheckedBadgeLabel: 'Verified by content hash',
  panelChooseFileToShare: 'Choose file to share',
  panelNoFileSelectedYet: 'No file selected yet.',
  panelClearSelectedFile: 'Clear',
  panelShare: 'Share',
  panelSharing: 'Sharing…',
  panelAdvancedPathSummary: 'Advanced: paste a daemon-readable path',
  panelPathPlaceholder: '/path/to/report.pdf',
  panelPathFieldLabel: 'File path to share',
  panelPathHint: 'Use this only for files already under the daemon data directory.',
  panelSharedInThisRoom: 'Shared in this room',
  panelAllFetchable: 'All fetchable',
  panelAwaitingProvider: (n, formatted = String(n)) => `${formatted} awaiting a provider`,
  panelHealthServingToPeers: 'Serving to peers',
  panelHealthFetchedLocally: 'Fetched locally',
  panelHealthSecurityCheckFailed: 'Security check failed',
  panelHealthFetchFailed: 'Fetch failed',
  panelHealthReadyToFetch: 'Ready to fetch',
  panelNProviders: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} provider` : `${formatted} providers`,
  pipeStateConnected: 'Connected',
  pipeStateOpen: 'Open',
  pipeStateClosed: 'Closed',
  panelExposeTitle: 'Expose a pipe',
  panelExposeCopy: 'Forward a local port to exactly one authorized peer.',
  panelTargetFieldLabel: 'Local target (host:port)',
  panelAuthorizedPeerLabel: 'Authorized peer',
  panelNoOtherMembers: 'no other members',
  panelPeerChoice: (name, role) => `${name} (${role})`,
  panelExpose: 'Expose',
  panelExposing: 'Exposing…',
  panelPipesEmpty: 'No pipes yet — expose a local port to one authorized peer above.',
  panelPipeMeta: 'by {openedBy} · authorized: {authorized}',
  panelConnect: 'Connect',
  panelConnecting: 'Connecting…',
  panelOpenPreview: 'Open preview ↗',
  panelClosePipe: 'Close',
  panelClosingPipe: 'Closing…',
  panelInspectorLabel: (tool) => `${tool} inspector`,

  // -- timeline ---------------------------------------------------------------
  timelineRoomTimeline: 'Room timeline',
  timelineEmptyState: 'No events yet — say something below.',
  timelineAgentChip: 'agent',
  timelineStatusFallback: 'status',
  timelineFileSharedMeta: '{sender} {role} shared a file · {time}',
  timelinePipeOpenedMeta: '{sender} {role} opened a pipe · {time}',
  timelineFileMeta: '{bytes} · {ext}',
  timelineOpenInPipes: 'Open in Pipes',
  timelineOpenInFiles: 'Open in Files',
  timelineAuthorizedPeer: 'authorized peer: {peer}',
  timelineSyslineRoomCreated: '{sender} created the room · {time}',
  timelineSyslineInvited: '{sender} invited {invitee} as {role} · {time}',
  timelineSyslineInvitedNoRole: '{sender} invited {invitee} · {time}',
  timelineSyslineJoined: '{who} joined as {role} · {time}',
  timelineSyslineLeft: '{who} left the room · {time}',
  timelineSyslinePipeClosed: '{sender} closed pipe {target} · {time}',
  timelineSomeone: 'someone',
  timelinePendingSending: 'Sending…',
  timelinePendingSyncing: 'Sent locally, syncing…',
  timelinePendingFailed: "Couldn't send",
  timelineRetryMessage: 'Retry sending message',
  timelineRetryMessageAt: (time) => `Retry sending your message from ${time}`,
  timelineNewMessages: (n, formatted = String(n)) =>
    n === 1 ? `${formatted} new message` : `${formatted} new messages`,
  timelineNewActivity: (n) => `${n} new activity`,
  timelineRunEvidence: (count, span) => `${count} updates · ${span}`,
  timelineRunShow: (count) => `Show ${count} updates`,
  timelineRunHide: 'Hide',
  timelineFilterActivity: 'Filter activity',
  timelineFilterConversation: 'Conversation',
  timelineFilterAgentRuns: 'Agent runs',
  timelineFilterMembership: 'Membership',
  timelineFilterFiles: 'Files',
  timelineFilterPipes: 'Pipes',
  timelineNoActivityMatches: 'No activity matches these filters.',

  // -- modals ------------------------------------------------------------------
  modalJoinCopy:
    'Paste the invite you received. A combined invite ({combined}) fills in the peer address automatically.',
  modalTicketLabel: 'Ticket',
  modalTicketPlaceholder: 'roomtkt1… or roomtkt1…#<endpoint_id>@host:port',
  modalPeerAddrLabel: 'Peer address',
  modalJoinSubmit: 'Join room',
  modalJoining: 'Joining…',
  modalJoinAttempt: (attempt, max, attemptText = String(attempt), maxText = String(max)) =>
    `Attempt ${attemptText}/${maxText}`,
  modalCreateTitle: 'Create a room',
  modalRoomNameLabel: 'Room name',
  modalRoomNamePlaceholder: 'Build Iroh Rooms MVP',
  modalCreating: 'Creating…',
  modalCreateHomonymWarning:
    'A room with that name already exists on this device — this one will get its own ID.',
  modalLeaveTitle: 'Leave room',
  modalLeaveCopy:
    'Leaving {room} {id} publishes a signed membership departure. This is different from closing the local ' +
    'session; you’ll need a new invite to join again.',
  modalLeaveSubmit: 'Leave room',
  modalLeaving: 'Leaving…',
  modalRenameTitle: 'Name this peer',
  modalRenameCopy: 'Local alias only — names never leave this machine.',
  modalRenameIdentityLabel: 'Identity:',
  modalRenameAliasLabel: 'Alias',
  modalRenameAliasPlaceholder: 'e.g. Maya R.',
  modalRenameClearAlias: 'Clear alias',

  // -- onboarding -------------------------------------------------------------
  onboardingTagline: 'Your rooms, your data. Private by default — built for humans & agents.',
  onboardingIdentityTitle: 'Create your identity',
  onboardingIdentityCopy1:
    'A keypair generated and stored by your local daemon. No account, no server — the private key never leaves this machine.',
  onboardingIdentityCopy2:
    "There's no password reset and no recovery — if you lose this device or its data folder, this identity is gone for good.",
  onboardingCreateIdentity: 'Create identity',
  onboardingCreatingIdentity: 'Creating…',
  onboardingYourIdentityId: 'Your identity ID',
  onboardingIdentityCardCopy1:
    'Being invited to a room? Send this ID to the inviter first — tickets are bound to it.',
  onboardingIdentityCardCopy2:
    'Peers show up by this same hex ID at first — click any name in a room to set a local nickname for them (only visible to you).',
  onboardingCreateRoomCopy: 'Start a space and invite people or agents with tickets.',
  onboardingJoinFinding: 'Finding the inviter and syncing the room invite…',
  onboardingJoinRetryingAttempt: (attempt, max, attemptText = String(attempt), maxText = String(max)) =>
    `Retrying join (${attemptText}/${maxText})…`,
  onboardingJoinRetryWait: (seconds, formatted = String(seconds)) =>
    `The first path did not answer. Retrying in ${formatted}s…`,

  // -- composer ---------------------------------------------------------------
  composerMessagePlaceholder: (roomName) => `Message ${roomName}`,
  composerSendMessage: 'Send message',
  composerShareAFile: 'Share a file',
  composerHint: 'Enter to send · Shift+Enter for a new line · ⎘ to share a file',
  composerSharingFile: 'Sharing file…',

  // -- formatting vocabulary ---------------------------------------------------
  formatToday: 'Today',
  formatYesterday: 'Yesterday',
  formatBytesB: (n) => `${n} B`,
  formatBytesKb: (n) => `${n} KB`,
  formatBytesMb: (n) => `${n} MB`,
  formatBytesGb: (n) => `${n} GB`,
  formatPercent: (n) => `${n}%`,
  formatJustNow: 'just now',
  formatMinutesAgo: (n) => `${n}m ago`,
  formatHoursAgo: (n) => `${n}h ago`,
  formatDaysAgo: (n) => `${n}d ago`,
};
