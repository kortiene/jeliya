/** The React catalog contract (issue #74).
 *
 *  The Flutter app resolves copy through a generated `AppStrings` class backed
 *  by an ARB catalog with a translator description per key. React gets the same
 *  contract, expressed in TypeScript rather than generated: `Catalog` is the
 *  shape, `en` is the source of truth, and every other locale is typed as
 *  `Catalog` — so a missing key is a COMPILE error, not a runtime blank.
 *
 *  Why not an i18n library. `ui/` ships exactly two runtime dependencies,
 *  react and react-dom, and `docs/room-workbench.md` records that adding one is
 *  a decision needing its own rationale — the repository went as far as
 *  hand-vendoring a QR encoder in both clients to avoid a dependency. Nothing
 *  here needs a runtime: message lookup is a property access, and every
 *  formatting concern the acceptance criteria name (dates, durations, counts,
 *  plurals, relative time) is already in the browser as `Intl`.
 *
 *  The seven rules this must honor are `docs/i18n.md`, which govern both
 *  clients. The two that shape this file:
 *
 *   - Rule 1: copy is resolved AT RENDER TIME (`useStrings()`), never captured
 *     into component state, or a locale switch would not reach it.
 *   - Rule 2: no sentence is assembled from fragments. A sentence with styled
 *     or interactive segments is ONE message with `{slot}` placeholders that
 *     the call site fills — see `Template` in `template.tsx`.
 */

/** A message with no placeholders. */
export type Message = string;

/** A message the call site completes. Kept as a function so the catalog itself
 *  states which values a translator may reorder, and so a missing argument is a
 *  type error rather than a `{name}` leaking onto the screen. */
export type MessageFn<A extends unknown[]> = (...args: A) => string;

/** Every user-visible string in the React client.
 *
 *  Names follow the Flutter scheme — `<area><Key>` in lowerCamelCase — so a
 *  reviewer comparing the two catalogs can line them up, and so the two clients
 *  cannot drift into different words for the same thing. The areas mirror the
 *  destinations in `docs/room-workbench.md`.
 *
 *  Ordered by area, and within an area by where the string appears on screen.
 */
export interface Catalog {
  /** BCP 47 tag this catalog is written in. Used for `<html lang>` and as the
   *  default formatting locale when the user has not chosen one separately. */
  readonly localeTag: string;

  // -- common ------------------------------------------------------------------
  commonRetry: Message;
  commonCancel: Message;
  commonClose: Message;
  commonClear: Message;
  commonSave: Message;
  commonBack: Message;
  commonCopy: Message;
  commonCopied: Message;
  commonCopyFailed: Message;
  commonReconnecting: Message;
  /** The em-dash-free "not available" placeholder for an absent value. */
  commonUnknown: Message;
  /** The parenthetical marking an input a user may leave blank. Rendered beside
   *  a field label, so it is a fragment of a LABEL, never of a sentence. */
  commonOptional: Message;
  /** Template for a field label and its optional marker, whose order may vary. */
  commonOptionalFieldLabel: Message;
  commonServing: Message;
  commonServingTooltip: Message;
  commonFileExtFallback: Message;
  commonTechnicalDetails: Message;
  commonTaskProgress: Message;
  commonChecking: Message;
  commonFetch: Message;
  commonFetching: Message;
  commonVerified: Message;
  commonFetched: Message;
  commonFailed: Message;
  commonRecheck: Message;
  commonOpenFile: Message;
  commonCopyPath: Message;
  commonCopySavedFilePath: Message;
  commonNoProviderOnline: Message;
  /** @param id the raw identity id shown in the same tooltip. */
  commonSetLocalNameFor: MessageFn<[id: string]>;

  // -- file fetching -----------------------------------------------------------
  fetchProvidersListedOnline: MessageFn<[n: number, formatted?: string]>;
  fetchProvidersListedOffline: MessageFn<[n: number, formatted?: string]>;
  fetchRecheckProvidersFor: MessageFn<[file: string]>;
  fetchFileNamed: MessageFn<[file: string]>;
  fetchOpenFileNamed: MessageFn<[file: string]>;
  fetchCopySavedPathFor: MessageFn<[file: string]>;
  fetchRetryNamed: MessageFn<[file: string]>;
  fetchVerifiedTooltip: MessageFn<[path: string]>;
  fetchFetchedTooltip: MessageFn<[path: string]>;
  /** `{bytes}` is formatted display text and `{path}` is rendered as code/link. */
  fetchDetailVerified: Message;
  /** `{bytes}` is formatted display text and `{path}` is rendered as code/link. */
  fetchDetailFetched: Message;
  fetchOpenLocalFileCopy: Message;
  fetchErrFileUnavailable: Message;
  fetchErrFileUnauthorized: Message;
  fetchErrHashMismatch: Message;

  // -- boot --------------------------------------------------------------------
  bootSyncing: Message;
  bootNotConnected: Message;
  bootContacting: Message;
  /** Two mono slots: `{daemon}` is the daemon binary name and `{port}` the port
   *  query parameter — both machine text (`tokens.ts`), so they are slots the
   *  sentence moves around rather than words to translate. */
  bootRetryingHint: Message;

  // -- shell / connection ------------------------------------------------------
  /** @param transport the daemon address the client is dialing, e.g.
   *         "ws://127.0.0.1:8080/ws". Never translated. */
  shellConnectionLost: MessageFn<[transport: string]>;
  shellDisconnected: Message;
  shellSkipToMain: Message;
  shellSkipToComposer: Message;
  /** The four connection states, as the rail's badge names them. Display labels
   *  for the wire `ConnectionState` — never the wire value itself (rule 3). */
  shellConnConnected: Message;
  shellConnConnecting: Message;
  shellConnReconnecting: Message;
  shellConnDisconnected: Message;
  /** Accessible names for the two primary navigations. They are distinct because
   *  the rail and the compact tab bar are both mounted on some shells, and two
   *  identically-named landmarks are useless to landmark navigation. */
  shellNavPrimary: Message;
  shellNavPrimaryMobile: Message;

  // -- global destinations (docs/room-workbench.md, decision 1) -----------------
  //
  // The rail entry, the compact tab and the page title all read these — one
  // destination is named ONCE, so a tab reading "Rooms" cannot open a page
  // titled something else.
  destRooms: Message;
  destFleet: Message;
  destSettings: Message;

  // -- room destinations -------------------------------------------------------
  roomDestActivity: Message;
  roomDestPeople: Message;
  roomDestAgents: Message;
  roomDestFiles: Message;
  roomDestPipes: Message;

  // -- rooms list --------------------------------------------------------------
  roomsYourRooms: Message;
  roomsChoose: Message;
  /** The rail's create button, its icon-button label, and the Create dialog's
   *  submit — one action, one name. */
  roomsCreate: Message;
  /** The rail's join button AND the Join dialog's title. */
  roomsJoinWithTicket: Message;
  roomsSearchPlaceholder: Message;
  roomsSearchLabel: Message;
  roomsFilterLegend: Message;
  roomsFilterAll: Message;
  roomsFilterActive: Message;
  roomsFilterDeparted: Message;
  roomsSectionPinned: Message;
  roomsSectionArchived: Message;
  /** @param n how many rooms the collapsed section holds. */
  roomsSectionCount: MessageFn<[n: number, formatted?: string]>;
  roomsEmpty: Message;
  /** @param query the text the user typed into the room search box. */
  roomsNoMatch: MessageFn<[query: string]>;
  roomsNoneInFilter: Message;
  roomsUnread: Message;
  /** @param n how many members the room's roster holds. Needs one/other in
   *  English; French agrees, and treats 0 as singular. */
  roomsMemberCount: MessageFn<[n: number, formatted?: string]>;
  roomsUntitled: Message;
  /** The settled status vocabulary (docs/room-workbench.md, decision 4).
   *  `Open`/`Closed` is whether THIS daemon holds a live session; `Left`/
   *  `Removed` is signed membership. Display labels for wire values, never the
   *  wire values themselves (rule 3). "Active" is retired and must not return. */
  roomsStateOpen: Message;
  roomsStateClosed: Message;
  roomsStateLeft: Message;
  roomsStateRemoved: Message;
  roomsSessionOpen: Message;
  /** Two separate keys, never one with a ternary slot: French needs different
   *  agreement and word order for each branch. They also title the room-recovery
   *  surfaces, which state the same signed fact — so it is worded once. */
  roomsYouLeft: Message;
  roomsYouWereRemoved: Message;
  /** @param room the room's display name, as the row shows it. */
  roomsPin: MessageFn<[room: string]>;
  /** @param room the room's display name, as the row shows it. */
  roomsUnpin: MessageFn<[room: string]>;
  /** @param room the room's display name, as the row shows it. */
  roomsArchive: MessageFn<[room: string]>;
  /** @param room the room's display name, as the row shows it. */
  roomsRestore: MessageFn<[room: string]>;
  /** The same four actions as a bare tooltip, with no room name in them. */
  roomsPinShort: Message;
  roomsUnpinShort: Message;
  roomsArchiveShort: Message;
  roomsRestoreShort: Message;
  /** Accessible name of the rail when it is a column beside the workspace
   *  rather than the page itself. */
  roomsRailLabel: Message;
  /** Accessible name of the room LIST navigation inside the rail — not the
   *  Rooms destination, which is `destRooms`. */
  roomsListLabel: Message;
  roomsProfile: Message;
  roomsProfileHandle: MessageFn<[id: string]>;

  // -- room recovery surfaces --------------------------------------------------
  roomNotOnDevice: Message;
  /** One slot: `{id}` is the room's short id, rendered mono. */
  roomNotOnDeviceDetail: Message;
  roomBackToRooms: Message;
  /** Two keys for the same reason `roomsYouLeft` is two: French says "votre
   *  départ" of a departure you chose and "votre retrait" of one you did not. */
  roomLeftDetail: Message;
  roomRemovedDetail: Message;

  // -- identity ----------------------------------------------------------------
  identitySelf: Message;
  identityP2P: Message;
  identityCopy: Message;
  /** @param id the endpoint id, shortened. Tier 2 — the `ep` prefix and the id
   *  itself stay verbatim in every locale. */
  identityEndpointShort: MessageFn<[id: string]>;
  /** @param id the full endpoint id, for the hover title. Tier 2. */
  identityEndpointTitle: MessageFn<[id: string]>;

  // -- device-local self label -------------------------------------------------
  selfLabelTitle: Message;
  selfLabelHint: Message;
  selfLabelPlaceholder: Message;

  // -- settings ----------------------------------------------------------------
  settingsTitle: Message;
  settingsLanguageLabel: Message;
  settingsFormattingLabel: Message;
  settingsLocaleSystemDefault: Message;
  settingsIdentityLabel: Message;
  settingsSelfLabelNote: Message;
  settingsEndpointLabel: Message;
  settingsDaemonLabel: Message;
  settingsSupportLabel: Message;
  settingsDiagnosticsTitle: Message;
  settingsDiagnosticsCopy: Message;
  settingsNoMessageBodies: Message;
  settingsNoInviteTickets: Message;
  settingsNoFileNamesOrPaths: Message;
  settingsNoFullIdentityIds: Message;
  settingsLastCapturedError: Message;
  settingsNoErrorCaptured: Message;
  settingsCopyDiagnostics: Message;
  settingsCopiedDiagnostics: Message;
  settingsReportIssue: Message;
  settingsIssueReportTitle: Message;

  // -- fleet ------------------------------------------------------------------
  fleetLivenessWorking: Message;
  fleetLivenessOnline: Message;
  fleetLivenessStale: Message;
  fleetLivenessOffline: Message;
  fleetSparkLoading: Message;
  fleetSparkEmpty: Message;
  fleetSparkEvents: MessageFn<[n: number, formatted?: string]>;
  fleetSparkNumericProgress: MessageFn<[n: number, formatted?: string]>;
  fleetSparkNoNumericProgress: Message;
  fleetCopyIdentityIdFor: MessageFn<[name: string]>;
  fleetLastStatusHint: Message;
  fleetLastStatus: MessageFn<[label: string]>;
  fleetNoStatusPosted: Message;
  fleetLastUpdate: MessageFn<[relativeTime: string]>;
  fleetNeverSeen: Message;
  fleetOpenRoom: Message;
  fleetCoverageEmpty: Message;
  fleetCoverage: MessageFn<[covered: string, total: string, percent: string]>;
  fleetAttentionFailed: Message;
  fleetAttentionReview: Message;
  fleetAttentionStale: Message;
  fleetAttentionOffline: Message;
  fleetNeedsAttention: Message;
  fleetNeedsAttentionEmpty: Message;
  fleetFilterAll: Message;
  fleetFilterLive: Message;
  fleetFilterNeedsAttention: Message;
  fleetFilterWorking: Message;
  fleetFilterOffline: Message;
  fleetSearchPlaceholder: Message;
  fleetSearchAgents: Message;
  fleetAddAgent: Message;
  fleetFilterAgents: Message;
  fleetLoadingAgents: Message;
  fleetEmptyNoAgents: Message;
  fleetEmptyNoMatch: Message;

  // -- add agent --------------------------------------------------------------
  addAgentTitle: Message;
  addAgentNoOwnedRooms: Message;
  addAgentIntro: Message;
  addAgentIntroEmphasis: Message;
  addAgentRoomLabel: Message;
  addAgentIdentityLabel: Message;
  addAgentIdentityPlaceholder: Message;
  addAgentWorkerLabel: Message;
  addAgentWorkerEchoOption: Message;
  addAgentWorkerClaudeOption: Message;
  addAgentClaudeWarning: Message;
  addAgentMintInvite: Message;
  addAgentMinting: Message;
  addAgentResultIntro: Message;
  addAgentLaunchCommandLabel: Message;
  addAgentCopyCommand: Message;
  addAgentGuidance: Message;
  addAgentTicketOnly: Message;
  addAgentCopyTicket: Message;
  addAgentNoDialableAddr: Message;
  addAgentNewInvite: Message;

  // -- invite -----------------------------------------------------------------
  inviteExpiry1h: Message;
  inviteExpiry24h: Message;
  inviteExpiry7d: Message;
  inviteExpiryNever: Message;
  inviteLifecycleJoined: Message;
  inviteLifecycleExpired: Message;
  inviteLifecycleWaiting: Message;
  inviteLifecycleJoinedCopy: Message;
  inviteLifecycleExpiredCopy: Message;
  inviteLifecycleWaitingCopy: Message;
  inviteExpiryErrorTitle: Message;
  inviteExpiryErrorMessage: Message;
  inviteExpiryErrorHint: Message;
  inviteShareTitle: Message;
  inviteTitle: Message;
  inviteReadyToSend: Message;
  inviteReadyToSendCopy: Message;
  inviteNoDialableAddress: Message;
  inviteNoDialableAddressCopy: Message;
  inviteCombinedCopy: Message;
  inviteTicketOnlyCopy: Message;
  inviteCombinedInviteLabel: Message;
  inviteInviteTicketLabel: Message;
  inviteCopyInvite: Message;
  inviteCopyTicket: Message;
  inviteShareInvite: Message;
  inviteShareTicket: Message;
  inviteQrLabel: Message;
  inviteQrCombinedCaption: Message;
  inviteQrTicketCaption: Message;
  inviteSeparatelySummary: Message;
  inviteCopyAddress: Message;
  inviteNoDialableAddressNote: Message;
  inviteGenerating: Message;
  inviteAgain: Message;
  inviteNewInvite: Message;
  inviteAlreadyInvited: Message;
  inviteIntro: Message;
  inviteRoomOpenForInviting: Message;
  inviteRoomOpenForInvitingCopy: Message;
  inviteInviteeIdentityId: Message;
  inviteInviteePlaceholder: Message;
  inviteIdentityInvalid: Message;
  inviteIdentityHint: Message;
  inviteRoleLabel: Message;
  inviteRoleMemberConsequence: Message;
  inviteRoleAgentConsequence: Message;
  inviteAgentWarning: Message;
  inviteTicketExpiryLabel: Message;
  inviteAdvancedExpiry: Message;
  inviteCustomExpiryLabel: Message;
  inviteCustomExpiryOverride: Message;
  inviteSendFresh: Message;
  inviteGenerateTicket: Message;

  // -- room header and inspector ---------------------------------------------
  roomNavLabel: Message;
  roomBackToActivity: Message;
  roomCloseInspector: Message;
  roomInformation: Message;
  roomInfoRoom: Message;
  roomInfoSession: Message;
  roomInfoAgents: Message;
  roomInfoInvites: Message;
  roomLoadingMembers: Message;
  commonMemberCount: MessageFn<[n: number, formatted?: string]>;
  roomHeaderAgentCount: MessageFn<[n: number, formatted?: string]>;
  roomHeaderInvitesPending: MessageFn<[n: number, formatted?: string]>;
  roomHeaderNoPeersConnected: Message;
  roomHeaderPeerToPeer: Message;
  roomHeaderRelayOnly: Message;
  roomHeaderPeerConnected: Message;
  roomHeaderPeerConnecting: Message;
  roomHeaderShareFile: Message;
  roomHeaderOpenPipe: Message;
  roomHeaderInvite: Message;
  roomHeaderPeerConnections: Message;
  roomHeaderPeerStateConnected: Message;
  roomHeaderPeerStateConnecting: Message;
  roomHeaderPeerStateOffline: Message;

  // -- People, Agents, Files, and Pipes inspector -----------------------------
  panelMembersEmpty: Message;
  panelRoomMemberCount: MessageFn<[n: number, formatted?: string]>;
  panelRosterCopy: Message;
  panelRoomRoster: Message;
  panelInvitedCount: MessageFn<[n: number, formatted?: string]>;
  panelThisDevice: Message;
  panelLeave: Message;
  panelOwnerStays: Message;
  panelOwnerStaysTitle: Message;
  panelAgentsEmpty: Message;
  panelNoStatusPostedYet: Message;
  panelAgentStatusFooter: MessageFn<[status: string]>;
  panelKindBinary: Message;
  panelKindText: Message;
  panelKindFile: Message;
  panelFilesHeroEmptyDetail: Message;
  panelFilesHeroDetail: MessageFn<[totalBytes: string, availableCount: number, formatted?: string]>;
  panelNFetched: MessageFn<[n: number, formatted?: string]>;
  panelServedByYou: MessageFn<[n: number, formatted?: string]>;
  panelNoSharedFilesYet: Message;
  panelSharedFileCount: MessageFn<[n: number, formatted?: string]>;
  panelFileAvailabilityLabel: Message;
  panelFetchableNow: Message;
  panelFetchableNowValue: MessageFn<[available: number, total: number, availableText?: string, totalText?: string]>;
  panelProviderDevices: Message;
  panelFilesShareToggle: Message;
  panelFilesShareToggleClose: Message;
  panelShareCardTitle: Message;
  panelShareCardHelp: Message;
  panelHashCheckedBadge: Message;
  panelHashCheckedBadgeLabel: Message;
  panelChooseFileToShare: Message;
  panelNoFileSelectedYet: Message;
  panelClearSelectedFile: Message;
  panelShare: Message;
  panelSharing: Message;
  panelAdvancedPathSummary: Message;
  panelPathPlaceholder: Message;
  panelPathFieldLabel: Message;
  panelPathHint: Message;
  panelSharedInThisRoom: Message;
  panelAllFetchable: Message;
  panelAwaitingProvider: MessageFn<[n: number, formatted?: string]>;
  panelHealthServingToPeers: Message;
  panelHealthFetchedLocally: Message;
  panelHealthSecurityCheckFailed: Message;
  panelHealthFetchFailed: Message;
  panelHealthReadyToFetch: Message;
  panelNProviders: MessageFn<[n: number, formatted?: string]>;
  pipeStateConnected: Message;
  pipeStateOpen: Message;
  pipeStateClosed: Message;
  panelExposeTitle: Message;
  panelExposeCopy: Message;
  panelTargetFieldLabel: Message;
  panelAuthorizedPeerLabel: Message;
  panelNoOtherMembers: Message;
  panelPeerChoice: MessageFn<[name: string, role: string]>;
  panelExpose: Message;
  panelExposing: Message;
  panelPipesEmpty: Message;
  panelPipeMeta: Message;
  panelConnect: Message;
  panelConnecting: Message;
  panelOpenPreview: Message;
  panelClosePipe: Message;
  panelClosingPipe: Message;
  panelInspectorLabel: MessageFn<[tool: string]>;

  // -- timeline ---------------------------------------------------------------
  timelineRoomTimeline: Message;
  timelineEmptyState: Message;
  timelineAgentChip: Message;
  timelineStatusFallback: Message;
  /** Whole event-heading templates. `{sender}`, optional `{role}`, and `{time}`
   *  are styled React slots; translators control their order. */
  timelineFileSharedMeta: Message;
  timelinePipeOpenedMeta: Message;
  /** A metadata line with already-formatted `{bytes}` and a file `{ext}`. */
  timelineFileMeta: Message;
  timelineOpenInPipes: Message;
  timelineOpenInFiles: Message;
  /** One `{peer}` slot, rendered as a peer name when one is known. */
  timelineAuthorizedPeer: Message;
  /** System-event sentences use templates because identity names are React
   *  nodes and translators must be free to move them. */
  timelineSyslineRoomCreated: Message;
  timelineSyslineInvited: Message;
  /** Invite event variant for an older/future payload that names no role. */
  timelineSyslineInvitedNoRole: Message;
  timelineSyslineJoined: Message;
  timelineSyslineLeft: Message;
  timelineSyslinePipeClosed: Message;
  timelineSomeone: Message;
  timelinePendingSending: Message;
  timelinePendingSyncing: Message;
  timelinePendingFailed: Message;
  timelineRetryMessage: Message;
  timelineRetryMessageAt: MessageFn<[time: string]>;
  /** @param n raw count for plural choice; @param formatted display count. */
  timelineNewMessages: MessageFn<[n: number, formatted?: string]>;
  /** Activity is a mass noun in both shipped locales. */
  timelineNewActivity: MessageFn<[n: string]>;
  timelineRunEvidence: MessageFn<[count: string, span: string]>;
  timelineRunShow: MessageFn<[count: string]>;
  timelineRunHide: Message;
  timelineFilterActivity: Message;
  timelineFilterConversation: Message;
  timelineFilterAgentRuns: Message;
  timelineFilterMembership: Message;
  timelineFilterFiles: Message;
  timelineFilterPipes: Message;
  timelineNoActivityMatches: Message;

  // -- modals (App.tsx) --------------------------------------------------------
  /** One slot: `{combined}` is the literal shape of a combined invite,
   *  `ticket#address`, rendered mono. A wire-format example, not a word. */
  modalJoinCopy: Message;
  modalTicketLabel: Message;
  modalTicketPlaceholder: Message;
  modalPeerAddrLabel: Message;
  modalJoinSubmit: Message;
  modalJoining: Message;
  /** @param attempt the join attempt now running, 1-based.
   *  @param max how many attempts the client will make in total. */
  modalJoinAttempt: MessageFn<[attempt: number, max: number, attemptText?: string, maxText?: string]>;
  modalCreateTitle: Message;
  modalRoomNameLabel: Message;
  modalRoomNamePlaceholder: Message;
  modalCreating: Message;
  modalCreateHomonymWarning: Message;
  modalLeaveTitle: Message;
  /** Two slots: `{room}` is the room's name (emphasised) and `{id}` its short id
   *  (mono). Both are always shown — the name alone cannot prove which room a
   *  signed, irreversible departure applies to. */
  modalLeaveCopy: Message;
  modalLeaveSubmit: Message;
  modalLeaving: Message;
  modalRenameTitle: Message;
  modalRenameCopy: Message;
  modalRenameIdentityLabel: Message;
  modalRenameAliasLabel: Message;
  modalRenameAliasPlaceholder: Message;
  modalRenameClearAlias: Message;

  // -- onboarding -------------------------------------------------------------
  onboardingTagline: Message;
  onboardingIdentityTitle: Message;
  onboardingIdentityCopy1: Message;
  onboardingIdentityCopy2: Message;
  onboardingCreateIdentity: Message;
  onboardingCreatingIdentity: Message;
  onboardingYourIdentityId: Message;
  onboardingIdentityCardCopy1: Message;
  onboardingIdentityCardCopy2: Message;
  onboardingCreateRoomCopy: Message;
  onboardingJoinFinding: Message;
  onboardingJoinRetryingAttempt: MessageFn<[
    attempt: number,
    max: number,
    attemptText?: string,
    maxText?: string,
  ]>;
  onboardingJoinRetryWait: MessageFn<[seconds: number, formatted?: string]>;

  // -- composer ---------------------------------------------------------------
  composerMessagePlaceholder: MessageFn<[roomName: string]>;
  composerSendMessage: Message;
  composerShareAFile: Message;
  composerHint: Message;
  composerSharingFile: Message;

  // -- formatting vocabulary (formats.ts, rule 4) ------------------------------
  //
  // These follow the TEXT locale, not the formatting locale: they are
  // vocabulary, and French writes octets where English writes bytes. Only the
  // NUMBER inside them comes from the formatting locale, already formatted —
  // which is why each takes a string, not a number.
  formatToday: Message;
  formatYesterday: Message;
  /** @param n a byte count, already formatted under the formatting locale. */
  formatBytesB: MessageFn<[n: string]>;
  /** @param n a kilobyte count, already formatted. French unit: Ko. */
  formatBytesKb: MessageFn<[n: string]>;
  /** @param n a megabyte count, already formatted. French unit: Mo. */
  formatBytesMb: MessageFn<[n: string]>;
  /** @param n a gigabyte count, already formatted. French unit: Go. */
  formatBytesGb: MessageFn<[n: string]>;
  /** @param n a percentage, already formatted. The SPACING is the localized
   *  part: French writes "42 %" with a narrow no-break space. */
  formatPercent: MessageFn<[n: string]>;
  formatJustNow: Message;
  /** Relative-time words follow the text catalog; only `{n}` follows the
   *  independently selected number-formatting locale. */
  formatMinutesAgo: MessageFn<[n: string]>;
  formatHoursAgo: MessageFn<[n: string]>;
  formatDaysAgo: MessageFn<[n: string]>;

  // -- wire enums and daemon errors ---------------------------------------------
  //
  // Declared here rather than in a separate fragment interface: the CI gate
  // (scripts/check-ui-i18n.mjs) reads this file to learn which keys exist, and a
  // fragment it cannot follow makes every key look undeclared — the parity rule
  // then reports hundreds of false findings and hides the real ones.
  // -- wire enums: roles, mid-sentence ----------------------------------------
  /** Role word for the protocol role `owner`, lowercase for mid-sentence use
   *  ("… joined as owner"). Capitalized pills use panelRoleOwner. The English
   *  happens to match the raw wire value; translate it as the natural word for
   *  a room owner — the wire value itself is never shown. */
  wireRoleOwnerInline: Message;
  /** Role word for the protocol role `member`, lowercase for mid-sentence use
   *  ("… joined as member"). Capitalized pills use panelRoleMember. */
  wireRoleMemberInline: Message;
  /** Role word for the protocol role `agent` (an automated participant),
   *  lowercase for mid-sentence use. Capitalized pills use panelRoleAgent. */
  wireRoleAgentInline: Message;

  // -- wire enums: roles, as a pill -------------------------------------------
  /** Display label for the room-owner role on roster rows and pickers.
   *  Translating it does not affect the wire role value. */
  panelRoleOwner: Message;
  /** Display label for the agent role (an automated member) on roster rows and
   *  pickers. Translating it does not affect the wire role value. */
  panelRoleAgent: Message;
  /** Display label for the ordinary member role on roster rows and pickers.
   *  Translating it does not affect the wire role value. */
  panelRoleMember: Message;

  // -- wire enums: member status ----------------------------------------------
  /** Roster status for an identity whose signed membership is current
   *  (protocol `active`). Display label only — deliberately NOT the word
   *  "Active", which the room rail uses for a live local session. */
  memberStatusMember: Message;
  /** Roster/agent-card status: invited, has not joined yet (protocol
   *  `invited`). Standalone capitalized word. */
  wireStatusInvited: Message;
  /** Roster/agent-card status: the member left the room voluntarily (protocol
   *  `left`, past tense of "to leave"). Standalone capitalized word. */
  wireStatusLeft: Message;
  /** Roster/agent-card status: the member was removed by someone else
   *  (protocol `removed`). Standalone capitalized word. */
  wireStatusRemoved: Message;
  /** Roster status shown when the daemon reports NO membership status at all.
   *  Says so rather than guessing — never rendered as a normal state. */
  memberStatusUnknown: Message;

  // -- wire enums: peer path (Tier 2 — see wireDisplay.ts) ---------------------
  /** Peer connection path in the room-header chip: traffic flows over a direct
   *  peer-to-peer connection. Lowercase, mid-phrase. `docs/glossary-fr.md`
   *  Tier 2: this is the daemon's own word — keep it verbatim in French. */
  wirePathDirect: Message;
  /** Peer connection path in the room-header chip: traffic is routed through a
   *  relay server rather than directly. Lowercase, mid-phrase. Tier 2 — keep it
   *  verbatim in French. */
  wirePathRelay: Message;

  // -- wire enums: daemon mode -------------------------------------------------
  /** Daemon mode in the Settings daemon summary: local-only loopback mode (no
   *  real networking; used for testing). Lowercase, mid-sentence. Prefer the
   *  established networking term in the target language. */
  wireModeLoopback: Message;
  /** Daemon mode in the Settings daemon summary: normal networked operation, as
   *  opposed to loopback test mode. Lowercase, mid-sentence. */
  wireModeReal: Message;

  // -- wire enums: client connection state, mid-sentence -----------------------
  /** Connection state, lowercase for mid-sentence use: the app is connected to
   *  its local daemon. Standalone capitalized badges use shellConn* instead. */
  wireConnConnectedInline: Message;
  /** Connection state, lowercase for mid-sentence use: the app is establishing
   *  its first connection to the local daemon. */
  wireConnConnectingInline: Message;
  /** Connection state, lowercase for mid-sentence use: the connection dropped
   *  and the app is trying to reconnect. */
  wireConnReconnectingInline: Message;
  /** Connection state, lowercase for mid-sentence use: the app has no
   *  connection to the local daemon. */
  wireConnDisconnectedInline: Message;

  // -- errors: peer_unreachable ------------------------------------------------
  /** Error headline: joining via an invite failed because the inviter (the room
   *  admin) could not be reached over the network in time. No blame. */
  errPeerUnreachableTitle: Message;
  /** Explanation: the invite itself parsed fine; the network hop to the room
   *  admin timed out. */
  errPeerUnreachableMessage: Message;
  /** Next step. "Combined invite" is a product term: an invite bundling the
   *  ticket with fresh network address info. */
  errPeerUnreachableAction: Message;

  // -- errors: bad_ticket ------------------------------------------------------
  /** Error headline: the invite's ticket is invalid for this user. "Invite" is
   *  the user-facing word; the credential inside it is called a ticket. */
  errBadTicketTitle: Message;
  /** Explanation for an unusable invite. "Identity" is the user's cryptographic
   *  identity on this device. */
  errBadTicketMessage: Message;
  /** Next step. "Identity ID" is the identifier shown in Settings — keep the
   *  term consistent with that screen. */
  errBadTicketAction: Message;

  // -- errors: ticket_expired --------------------------------------------------
  /** Error headline: the ticket passed its expiry time and the room rejected
   *  it. */
  errTicketExpiredTitle: Message;
  /** Explanation for an expired invite. */
  errTicketExpiredMessage: Message;
  /** Next step: request a newly generated ticket from whoever invited you. */
  errTicketExpiredAction: Message;

  // -- errors: room_not_open ---------------------------------------------------
  /** Error headline: the action requires a live room session first.
   *  Imperative, instructional tone. */
  errRoomNotOpenTitle: Message;
  /** Explanation. "Daemon" is a never-translate term (the local background
   *  process, jeliyad). */
  errRoomNotOpenMessage: Message;
  /** Next step: three sequential instructions, performed in order. */
  errRoomNotOpenAction: Message;

  // -- errors: not_a_member ----------------------------------------------------
  /** Error headline: the room's records do not list this user as an active
   *  member, so the action was refused. */
  errNotAMemberTitle: Message;
  /** Explanation. "Signed room history" is the cryptographically signed
   *  membership log. */
  errNotAMemberMessage: Message;
  /** Next step: two alternatives joined with "or". */
  errNotAMemberAction: Message;

  // -- errors: room_unknown ----------------------------------------------------
  /** Error headline: this device has no local copy of the room's history.
   *  "Local" means stored on this device. */
  errRoomUnknownTitle: Message;
  /** Explanation. "Daemon" is never translated. */
  errRoomUnknownMessage: Message;
  /** Next step. "Peer hint" is a product term: address information for a
   *  reachable member that helps the device find the room. */
  errRoomUnknownAction: Message;

  // -- errors: file_unauthorized -----------------------------------------------
  /** Error headline: a file download was refused — this user is not authorized
   *  for the file. */
  errFileUnauthorizedTitle: Message;
  /** Explanation. "Provider" is a product term: a device that can serve the
   *  file's bytes. */
  errFileUnauthorizedMessage: Message;
  /** Next step: two alternatives from the sender, then retry. */
  errFileUnauthorizedAction: Message;

  // -- errors: hash_mismatch ---------------------------------------------------
  /** Error headline: downloaded bytes failed the cryptographic hash check.
   *  Serious security tone. */
  errHashMismatchTitle: Message;
  /** Explanation. "Hard stop" means the app refuses to proceed at all; the
   *  downloaded copy is deleted and never displayed. Contains an em dash. */
  errHashMismatchMessage: Message;
  /** Next step. The second sentence is a firm prohibition — retrying the same
   *  copy would fail again and could be unsafe. */
  errHashMismatchAction: Message;

  // -- errors: connection_lost -------------------------------------------------
  /** Error headline: the app lost its connection to the local background
   *  process. "Daemon" is never translated. */
  errConnectionLostTitle: Message;
  /** Explanation. "jeliyad" is the daemon binary's name — never translate, keep
   *  the exact lowercase spelling. */
  errConnectionLostMessage: Message;
  /** Next step: the app reconnects automatically; the user waits and retries. */
  errConnectionLostAction: Message;

  // -- errors: invalid_params --------------------------------------------------
  /** Error headline: the daemon rejected the request because one of its input
   *  values was invalid (usually something the user typed). */
  errInvalidParamsTitle: Message;
  /** Explanation for an invalid request. */
  errInvalidParamsMessage: Message;
  /** Next step: re-check the entered values and retry. */
  errInvalidParamsAction: Message;

  // -- errors: identity_missing ------------------------------------------------
  /** Error headline: the action needs a user identity but none has been created
   *  on this device yet. */
  errIdentityMissingTitle: Message;
  /** Explanation. "Here" means on this device/daemon. */
  errIdentityMissingMessage: Message;
  /** Next step: create the identity (onboarding/Settings) before retrying. */
  errIdentityMissingAction: Message;

  // -- errors: identity_exists -------------------------------------------------
  /** Error headline: identity creation was refused because this device's daemon
   *  already holds one identity (only one is allowed). */
  errIdentityExistsTitle: Message;
  /** Explanation for the duplicate-identity error. Contains an em dash. */
  errIdentityExistsMessage: Message;
  /** Next step. "Settings" names the app's Settings screen — keep consistent
   *  with that screen's title. */
  errIdentityExistsAction: Message;

  // -- errors: file_unavailable ------------------------------------------------
  /** Error headline: the file cannot be downloaded right now because no device
   *  holding it is online. Temporary condition, calm tone. */
  errFileUnavailableTitle: Message;
  /** Explanation. "Provider" is a product term: a device that can serve the
   *  file's bytes. */
  errFileUnavailableMessage: Message;
  /** Next step: try again later, once the sender's device is online. */
  errFileUnavailableAction: Message;

  // -- errors: file_too_large --------------------------------------------------
  /** Error headline: the user tried to share a file above the size limit. */
  errFileTooLargeTitle: Message;
  /** Explanation. "100 MiB" is the fixed limit — keep the number and the
   *  mebibyte unit "MiB" exactly as-is. */
  errFileTooLargeMessage: Message;
  /** Next step: two alternatives, joined with "or". */
  errFileTooLargeAction: Message;

  // -- errors: file_unreadable -------------------------------------------------
  /** Error headline: the file the user picked to share could not be read from
   *  local disk. */
  errFileUnreadableTitle: Message;
  /** Explanation. "Picked" refers to the file chosen in the file picker. */
  errFileUnreadableMessage: Message;
  /** Next step: verify it still exists with read permission, then retry. */
  errFileUnreadableAction: Message;

  // -- errors: pipe_denied -----------------------------------------------------
  /** Error headline: access to a pipe was refused. "Pipe" is a never-translate
   *  protocol term (a named byte-stream endpoint shared between peers). */
  errPipeDeniedTitle: Message;
  /** Explanation for denied pipe access. */
  errPipeDeniedMessage: Message;
  /** Next step. "Expose" is the product verb for granting a pipe to an
   *  identity; "pipe" is never translated. */
  errPipeDeniedAction: Message;

  // -- errors: internal --------------------------------------------------------
  /** Error headline for an internal (unclassified) daemon failure. */
  errInternalTitle: Message;
  /** Explanation: even the daemon does not know a more specific cause. */
  errInternalMessage: Message;
  /** Next step. "Settings" names the app's Settings screen, which has a
   *  copy-diagnostics feature — keep consistent with that screen's title. */
  errInternalAction: Message;

  // -- errors: unknown / future codes ------------------------------------------
  /** Fallback headline for any error code this app version has no specific copy
   *  for (unknown or future codes). */
  errUnknownTitle: Message;
  /** Fallback explanation. The raw error stays visible in the card's collapsed
   *  "Technical details" section. */
  errUnknownMessage: Message;
  /** Fallback next step. "Technical details" names the collapsed section on the
   *  same error card — it must match that section's label exactly. */
  errUnknownAction: Message;
}

/** A locale's catalog. Typed as the full `Catalog`, so TypeScript rejects an
 *  incomplete translation before the CI completeness gate ever runs — the gate
 *  exists for what types cannot see (an empty string, a key left in English). */
export type LocaleCatalog = Catalog;
