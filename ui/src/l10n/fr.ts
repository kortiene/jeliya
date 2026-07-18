/** French — `docs/glossary-fr.md`, decision 7 (typographie), Tier 1–3 vocabulary.
 *
 *  Typographie (non-negotiable, `docs/glossary-fr.md` decision 7). The two
 *  invisible spaces are written as ESCAPES, never as literal characters, so a
 *  reviewer can see the contract in the diff instead of trusting a glyph they
 *  cannot see:
 *
 *    \u202f  narrow no-break space — before `; ! ?` and inside « guillemets »
 *    \u00a0  no-break space — before `:`
 *    U+2019 `’` and U+2026 `…` are typed directly: they are visible, and are
 *    already the English catalog's norm.
 *
 *  Two places where the Flutter `app_fr.arb` does NOT meet decision 7 and this
 *  catalog does: the guillemets in `roomsNoMatch` (Flutter's
 *  `sidebarNoRoomsMatch` has no inner narrow spaces) and the percent sign in
 *  `formatPercent` (Flutter's `commonPercent` uses a plain space). Both are
 *  worth fixing there; neither is worth copying here.
 *
 *  Register: vouvoiement, sentence case (never Title Case), accents kept on
 *  capitals, calm and concrete — the honest tone of the English catalog, not a
 *  marketing one.
 *
 *  Vocabulary: Tier 1 is translated consistently (room → salon, member →
 *  membre, settings → réglages, Your Rooms → Vos salons). Tier 2 is verbatim in
 *  every locale — `direct`/`relay`, error codes, `daemon`, `jeliyad`, `pipe`,
 *  endpoint and identity ids — and most of it lives in `tokens.ts`, outside this
 *  file entirely. Tier 3, the brand, is never translated.
 *
 *  Where the Flutter catalog already translates a string, its French is REUSED
 *  verbatim. The two clients must not ship two French words for one English one.
 */

import type { LocaleCatalog } from './catalog';

export const fr: LocaleCatalog = {
  // -- wire enums and daemon errors ---------------------------------------------
  //
  // Inlined rather than spread from another module ON PURPOSE: the CI gate
  // (scripts/check-ui-i18n.mjs) reads these files with a restricted scanner, and
  // a spread it cannot follow makes the parity, emptiness and typography rules
  // silently stop running — a gate that reports nothing looks identical to a gate
  // that finds nothing. One locale, one file, every value visible.
  wireRoleOwnerInline: 'propriétaire',
  wireRoleMemberInline: 'membre',
  wireRoleAgentInline: 'agent',

  panelRoleOwner: 'Propriétaire',
  panelRoleAgent: 'Agent',
  panelRoleMember: 'Membre',

  memberStatusMember: 'Membre',
  wireStatusInvited: 'Invité',
  wireStatusLeft: 'Parti',
  wireStatusRemoved: 'Exclu',
  memberStatusUnknown: 'Inconnu',

  // Tier 2 (docs/glossary-fr.md): the daemon's own word, never translated.
  wirePathDirect: 'direct',
  wirePathRelay: 'relay',

  wireModeLoopback: 'loopback',
  wireModeReal: 'réel',

  wireConnConnectedInline: 'connecté',
  wireConnConnectingInline: 'en cours de connexion',
  wireConnReconnectingInline: 'en cours de reconnexion',
  wireConnDisconnectedInline: 'déconnecté',

  errPeerUnreachableTitle: 'Impossible de joindre l’invitant',
  errPeerUnreachableMessage:
    'L’invitation est lisible, mais cet appareil n’a pas pu joindre l’administrateur du salon à temps.',
  errPeerUnreachableAction:
    'Demandez à l’invitant de garder le salon ouvert, puis réessayez. Une nouvelle invitation combinée peut aider si l’adresse a changé.',

  errBadTicketTitle: 'Cette invitation ne peut pas être utilisée',
  errBadTicketMessage:
    'Le ticket est invalide pour cette identité, mal formé, ou ne correspond plus à l’invitation du salon.',
  errBadTicketAction: 'Demandez une nouvelle invitation générée pour votre identifiant d’identité actuel.',

  errTicketExpiredTitle: 'Cette invitation a expiré',
  errTicketExpiredMessage: 'Le salon a rejeté le ticket parce que sa date d’expiration est dépassée.',
  errTicketExpiredAction: 'Demandez à l’invitant de générer un nouveau ticket.',

  errRoomNotOpenTitle: 'Ouvrez d’abord le salon',
  errRoomNotOpenMessage: 'Cette action nécessite une session de salon active sur votre daemon.',
  errRoomNotOpenAction: 'Ouvrez le salon, attendez la fin de la synchronisation, puis réessayez.',

  errNotAMemberTitle: 'Vous n’êtes pas membre actif',
  errNotAMemberMessage:
    'L’historique signé du salon n’admet pas actuellement cette identité comme membre actif.',
  errNotAMemberAction:
    'Utilisez une invitation valide pour cette identité, ou demandez au propriétaire du salon de vous ajouter à nouveau.',

  errRoomUnknownTitle: 'Ce salon n’est pas encore sur cet appareil',
  errRoomUnknownMessage: 'Le daemon ne dispose pas de suffisamment d’historique pour ouvrir ce salon.',
  errRoomUnknownAction:
    'Rejoignez le salon avec une invitation, ou ouvrez-le avec un indice de pair joignable.',

  errFileUnauthorizedTitle: 'Accès non autorisé à ce fichier',
  errFileUnauthorizedMessage:
    'Tous les fournisseurs joignables ont refusé le transfert, car l’historique signé n’admet pas cette identité pour ce fichier.',
  errFileUnauthorizedAction:
    'Demandez à l’expéditeur de partager à nouveau le fichier ou de vous réinviter, puis réessayez.',

  errHashMismatchTitle: 'Échec du contrôle de sécurité',
  errHashMismatchMessage:
    'Les octets récupérés ne correspondent pas à l’empreinte du fichier. C’est un arrêt définitif — la copie est supprimée, jamais affichée.',
  errHashMismatchAction:
    'Demandez à l’expéditeur de partager à nouveau le fichier. Ne réessayez pas avec la même copie.',

  errConnectionLostTitle: 'Connexion au daemon perdue',
  errConnectionLostMessage: 'L’interface locale n’est pas connectée à jeliyad pour le moment.',
  errConnectionLostAction: 'Attendez la reconnexion, puis réessayez l’action.',

  errInvalidParamsTitle: 'Cette requête n’était pas valide',
  errInvalidParamsMessage: 'Le daemon a rejeté l’une des valeurs de cette requête.',
  errInvalidParamsAction: 'Vérifiez ce que vous avez saisi, puis réessayez.',

  errIdentityMissingTitle: 'Pas encore d’identité sur ce daemon',
  errIdentityMissingMessage: 'Cette action nécessite votre identité, et aucune n’a encore été créée ici.',
  errIdentityMissingAction: 'Créez d’abord votre identité, puis réessayez.',

  errIdentityExistsTitle: 'Une identité existe déjà',
  errIdentityExistsMessage: 'Ce daemon détient déjà une identité — il est impossible d’en créer une seconde.',
  errIdentityExistsAction: 'Utilisez l’identité existante affichée dans Réglages.',

  errFileUnavailableTitle: 'Fichier indisponible pour le moment',
  errFileUnavailableMessage: 'Aucun fournisseur n’est encore en ligne pour ce fichier.',
  errFileUnavailableAction: 'Revérifiez quand l’expéditeur sera de nouveau en ligne.',

  errFileTooLargeTitle: 'Ce fichier est trop volumineux pour être partagé',
  errFileTooLargeMessage: 'Les partages sont limités à 100 MiB par fichier.',
  errFileTooLargeAction: 'Choisissez un fichier plus petit, ou divisez le contenu.',

  errFileUnreadableTitle: 'Ce fichier n’a pas pu être lu',
  errFileUnreadableMessage: 'Le fichier choisi n’a pas pu être ouvert depuis le disque.',
  errFileUnreadableAction: 'Vérifiez que le fichier existe toujours et qu’il est lisible, puis réessayez.',

  errPipeDeniedTitle: 'Accès au pipe refusé',
  errPipeDeniedMessage: 'Ce pipe n’autorise pas votre identité.',
  errPipeDeniedAction: 'Demandez au propriétaire du pipe de l’exposer à votre identité.',

  errInternalTitle: 'Le daemon a rencontré une défaillance inattendue',
  errInternalMessage: 'Cette requête a échoué pour une raison que le daemon n’a pas pu classer.',
  // U+202F before the semicolon (decision 7).
  errInternalAction:
    'Réessayez ; si l’échec persiste, copiez les diagnostics depuis Réglages et signalez le problème.',

  errUnknownTitle: 'Une erreur s’est produite',
  errUnknownMessage:
    'Le daemon a signalé une erreur pour laquelle cette application n’a pas de message spécifique.',
  // U+202F inside the guillemets (decision 7) — « Détails techniques » must
  // match the disclosure's own label.
  errUnknownAction:
    'Ouvrez « Détails techniques » pour voir l’erreur exacte, puis réessayez.',

  localeTag: 'fr',

  // -- common ------------------------------------------------------------------
  commonRetry: 'Réessayer',
  commonCancel: 'Annuler',
  commonClose: 'Fermer',
  commonClear: 'Effacer',
  commonSave: 'Enregistrer',
  commonBack: 'Retour',
  commonCopy: 'Copier',
  commonCopied: 'Copié ✓',
  commonCopyFailed: 'Impossible de copier — sélectionnez le texte et copiez-le manuellement.',
  commonReconnecting: 'Reconnexion…',
  commonUnknown: 'Inconnu',
  commonOptional: '(facultative)',
  commonOptionalFieldLabel: '{label} {optional}',
  commonServing: 'Diffusion',
  commonServingTooltip: 'Ce daemon diffuse déjà ce fichier aux pairs.',
  commonFileExtFallback: 'fichier',
  commonTechnicalDetails: 'Détails techniques',
  commonTaskProgress: 'Progression de la tâche',
  commonChecking: 'Vérification…',
  commonFetch: 'Récupérer',
  commonFetching: 'Récupération…',
  commonVerified: '✓ Vérifié',
  commonFetched: '✓ Récupéré',
  commonFailed: '✕ Échec',
  commonRecheck: 'Revérifier',
  commonOpenFile: 'Ouvrir le fichier',
  commonCopyPath: 'Copier le chemin',
  commonCopySavedFilePath: 'Copier le chemin du fichier enregistré',
  commonNoProviderOnline: 'Aucun fournisseur en ligne',
  commonSetLocalNameFor: (id) => `${id}\nCliquez pour définir un nom local`,

  // -- file fetching -----------------------------------------------------------
  fetchProvidersListedOnline: (n, formatted = String(n)) =>
    `${formatted} ${n < 2 ? 'fournisseur indiqué' : 'fournisseurs indiqués'}\u202f; au moins un est en ligne`,
  fetchProvidersListedOffline: (n, formatted = String(n)) =>
    `${formatted} ${n < 2 ? 'fournisseur indiqué' : 'fournisseurs indiqués'}\u202f; aucun n’est en ligne pour le moment`,
  fetchRecheckProvidersFor: (file) => `Revérifier les fournisseurs de ${file}`,
  fetchFileNamed: (file) => `Récupérer ${file}`,
  fetchOpenFileNamed: (file) => `Ouvrir le fichier ${file}`,
  fetchCopySavedPathFor: (file) => `Copier le chemin enregistré de ${file}`,
  fetchRetryNamed: (file) => `Réessayer de récupérer ${file}`,
  fetchVerifiedTooltip: (path) => `vérifié · ${path}`,
  fetchFetchedTooltip: (path) => `récupéré · ${path}`,
  fetchDetailVerified: 'Vérifié · {bytes} · enregistré dans {path}',
  fetchDetailFetched: 'Récupéré · {bytes} · enregistré dans {path}',
  fetchOpenLocalFileCopy: 'Ouvrir la copie locale du fichier',
  fetchErrFileUnavailable:
    'Pour l’instant, aucun fournisseur n’est en ligne pour ce fichier. Revérifiez quand l’expéditeur sera de nouveau en ligne.',
  fetchErrFileUnauthorized:
    'Tous les fournisseurs ont refusé cette récupération — votre identité n’est pas autorisée à y accéder. Demandez à l’expéditeur de partager à nouveau ou de vous réinviter.',
  fetchErrHashMismatch:
    'Ce fichier a échoué à un contrôle de sécurité et n’a pas été enregistré — il a peut-être été corrompu ou altéré pendant le transfert.',

  // -- boot --------------------------------------------------------------------
  bootSyncing: 'Synchronisation…',
  bootNotConnected: 'Non connecté.',
  bootContacting: 'Connexion au daemon…',
  bootRetryingHint: 'Nouvelles tentatives avec délai progressif — lancez {daemon} ou passez {port}.',

  // -- shell / connection ------------------------------------------------------
  shellConnectionLost: (transport) => `Connexion au daemon perdue — reconnexion… (${transport})`,
  shellDisconnected: 'Déconnecté du daemon.',
  shellSkipToMain: 'Aller au contenu principal',
  shellSkipToComposer: 'Aller au champ de message',
  shellConnConnected: 'Connecté',
  shellConnConnecting: 'Connexion…',
  shellConnReconnecting: 'Reconnexion…',
  shellConnDisconnected: 'Déconnecté',
  shellNavPrimary: 'Principal',
  shellNavPrimaryMobile: 'Principal (mobile)',

  // -- global destinations -----------------------------------------------------
  destRooms: 'Salons',
  destFleet: 'Flotte d’agents',
  destSettings: 'Réglages',

  // -- room destinations -------------------------------------------------------
  roomDestActivity: 'Activité',
  roomDestPeople: 'Personnes',
  roomDestAgents: 'Agents et exécutions',
  roomDestFiles: 'Fichiers',
  roomDestPipes: 'Pipes',

  // -- rooms list --------------------------------------------------------------
  roomsYourRooms: 'Vos salons',
  roomsChoose: 'Choisissez un salon.',
  roomsCreate: 'Créer un salon',
  roomsJoinWithTicket: 'Rejoindre avec un ticket',
  roomsSearchPlaceholder: 'Rechercher des salons…',
  roomsSearchLabel: 'Rechercher des salons par nom ou identifiant court',
  roomsFilterLegend: 'Filtrer les salons par cycle de vie',
  roomsFilterAll: 'Tous',
  roomsFilterActive: 'Actifs',
  roomsFilterDeparted: 'Quittés et retirés',
  roomsSectionPinned: 'Épinglés',
  roomsSectionArchived: 'Archivés',
  roomsSectionCount: (n, formatted = String(n)) => `(${formatted})`,
  roomsEmpty: 'Aucun salon pour l’instant',
  // Guillemets with their inner narrow no-break spaces (decision 7). The Flutter
  // catalog's `sidebarNoRoomsMatch` is missing them; this follows the contract.
  roomsNoMatch: (query) => `Aucun salon ne correspond à «\u202f${query}\u202f».`,
  roomsNoneInFilter: 'Aucun salon dans ce filtre.',
  roomsUnread: 'Non lu',
  // French treats 0 as singular, unlike English: « 0 membre », « 1 membre »,
  // « 2 membres ». Pinned the same way the Flutter `strings_fr_test` pins it.
  roomsMemberCount: (n, formatted = String(n)) =>
    n < 2 ? `${formatted} membre` : `${formatted} membres`,
  roomsUntitled: 'Salon sans titre',
  roomsStateOpen: 'Ouvert',
  roomsStateClosed: 'Fermé',
  roomsStateLeft: 'Quitté',
  roomsStateRemoved: 'Retiré',
  roomsSessionOpen: 'Session ouverte',
  roomsYouLeft: 'Vous avez quitté ce salon',
  roomsYouWereRemoved: 'Vous avez été retiré de ce salon',
  roomsPin: (room) => `Épingler ${room}`,
  roomsUnpin: (room) => `Désépingler ${room}`,
  roomsArchive: (room) => `Archiver ${room}`,
  roomsRestore: (room) => `Restaurer ${room}`,
  roomsPinShort: 'Épingler',
  roomsUnpinShort: 'Désépingler',
  roomsArchiveShort: 'Archiver',
  roomsRestoreShort: 'Restaurer depuis l’archive',
  roomsRailLabel: 'Barre latérale des salons',
  roomsListLabel: 'Salons',
  roomsProfile: 'Profil et réglages',
  roomsProfileHandle: (id) => `@${id}`,

  // -- room recovery surfaces --------------------------------------------------
  roomNotOnDevice: 'Ce salon n’est pas sur cet appareil',
  roomNotOnDeviceDetail:
    'Rien ici ne correspond à {id}. Il se trouve peut-être sur un autre appareil, ou vous ne l’avez pas ' +
    'encore rejoint.',
  roomBackToRooms: 'Retour aux salons',
  roomLeftDetail:
    'Votre départ est publié dans le journal signé du salon. Il vous faudra une nouvelle invitation pour le ' +
    'rejoindre.',
  roomRemovedDetail:
    'Votre retrait est publié dans le journal signé du salon. Il vous faudra une nouvelle invitation pour le ' +
    'rejoindre.',

  // -- identity ----------------------------------------------------------------
  identitySelf: 'Vous',
  identityP2P: 'Identité P2P',
  identityCopy: 'Copier l’identifiant d’identité',
  // Tier 2: `ep` and `endpoint` are the daemon's own words for a wire id.
  identityEndpointShort: (id) => `ep ${id}`,
  identityEndpointTitle: (id) => `endpoint ${id}`,

  // -- device-local self label -------------------------------------------------
  selfLabelTitle: 'Votre nom sur cet appareil',
  selfLabelHint: 'Visible uniquement par vous — jamais partagé ni signé.',
  selfLabelPlaceholder: 'p. ex. Alex',

  // -- settings ----------------------------------------------------------------
  settingsTitle: 'Réglages',
  settingsLanguageLabel: 'Langue',
  settingsFormattingLabel: 'Dates et nombres',
  settingsLocaleSystemDefault: 'Par défaut du système',
  settingsIdentityLabel: 'Identité P2P',
  settingsSelfLabelNote:
    'Votre nom est une étiquette locale — il ne modifie jamais votre identité cryptographique, qui est irrécupérable si cet appareil ou son dossier de données est perdu.',
  settingsEndpointLabel: 'Point de terminaison',
  settingsDaemonLabel: 'Daemon',
  settingsSupportLabel: 'Assistance',
  settingsDiagnosticsTitle: 'Diagnostics',
  settingsDiagnosticsCopy:
    'Copiez un instantané respectueux de la vie privée pour les rapports de bug : version du daemon, état de la connexion, nombre de salons, état des pairs, transferts de fichiers, pipes et dernière erreur de l’interface.',
  settingsNoMessageBodies: 'Aucun corps de message',
  settingsNoInviteTickets: 'Aucun ticket d’invitation',
  settingsNoFileNamesOrPaths: 'Aucun nom de fichier ni chemin local complet',
  settingsNoFullIdentityIds: 'Aucun identifiant d’identité complet',
  settingsLastCapturedError: 'Dernière erreur capturée',
  settingsNoErrorCaptured: 'Aucune erreur d’action de l’interface capturée pendant cette session.',
  settingsCopyDiagnostics: 'Copier les diagnostics',
  settingsCopiedDiagnostics: 'Diagnostics copiés',
  settingsReportIssue: 'Signaler un problème',
  settingsIssueReportTitle: 'Signalement de problème Jeliya',

  // -- fleet ------------------------------------------------------------------
  fleetLivenessWorking: 'Au travail',
  fleetLivenessOnline: 'En ligne',
  fleetLivenessStale: 'Sans nouvelles',
  fleetLivenessOffline: 'Hors ligne',
  fleetSparkLoading: 'Chargement de l’historique des statuts',
  fleetSparkEmpty: 'Aucun historique de statuts pour l’instant',
  fleetSparkEvents: (n, formatted = String(n)) =>
    n === 0 || n === 1
      ? `${formatted} événement de statut`
      : `${formatted} événements de statut`,
  fleetSparkNumericProgress: (n, formatted = String(n)) =>
    `dont ${formatted} avec une progression numérique`,
  fleetSparkNoNumericProgress: 'aucune progression numérique',
  fleetCopyIdentityIdFor: (name) => `Copier l’identifiant d’identité de ${name}`,
  fleetLastStatusHint: 'Dernier statut publié — son état de présence ne le confirme plus',
  fleetLastStatus: (label) => `Dernier\u00a0: ${label}`,
  fleetNoStatusPosted: 'Aucun statut publié pour l’instant.',
  fleetLastUpdate: (relativeTime) => `Dernière mise à jour ${relativeTime}`,
  fleetNeverSeen: 'Jamais vu',
  fleetOpenRoom: 'Ouvrir le salon',
  fleetCoverageEmpty: 'Couverture des salons\u00a0: aucun salon pour l’instant.',
  fleetCoverage: (covered, total, percent) =>
    `Salons avec un agent\u00a0: ${covered} sur ${total} (${percent}).`,
  fleetAttentionFailed: 'Échec',
  fleetAttentionReview: 'En attente de revue',
  fleetAttentionStale: 'Sans nouvelles',
  fleetAttentionOffline: 'Hors ligne après travail',
  fleetNeedsAttention: 'Attention requise',
  fleetNeedsAttentionEmpty: 'Rien ne requiert d’attention pour l’instant.',
  fleetFilterAll: 'Tous',
  fleetFilterLive: 'En ligne',
  fleetFilterNeedsAttention: 'Attention requise',
  fleetFilterWorking: 'Au travail',
  fleetFilterOffline: 'Hors ligne',
  fleetSearchPlaceholder: 'Rechercher des agents…',
  fleetSearchAgents: 'Rechercher des agents',
  fleetAddAgent: '＋ Ajouter un agent',
  fleetFilterAgents: 'Filtrer les agents',
  fleetLoadingAgents: 'Chargement des agents',
  fleetEmptyNoAgents:
    'Aucun salon ne contient d’agent pour l’instant. Utilisez «\u202fAjouter un agent\u202f» pour émettre une invitation.',
  fleetEmptyNoMatch: 'Aucun agent ne correspond à ce filtre.',

  // -- add agent --------------------------------------------------------------
  addAgentTitle: 'Ajouter un agent',
  addAgentNoOwnedRooms:
    'Vous ne possédez encore aucun salon. Créez d’abord un salon — les invitations d’agent ne peuvent être émises que pour un salon que vous possédez.',
  addAgentIntro:
    'Émettez un ticket de rôle agent pour un salon que vous possédez. Cette opération {emphasis} — exécuter la commande ci-dessous sur la machine de l’agent est une étape humaine et délibérée (la frontière de sécurité).',
  addAgentIntroEmphasis: 'ne démarre rien',
  addAgentRoomLabel: 'Salon',
  addAgentIdentityLabel: 'Identifiant d’identité de l’agent',
  addAgentIdentityPlaceholder:
    'Identifiant d’identité de 64 caractères hexadécimaux (fourni par jeliya-agent.mjs --identity-only)',
  addAgentWorkerLabel: 'Worker',
  addAgentWorkerEchoOption: 'echo (sans danger — aucune exécution réelle, pour tester la procédure)',
  addAgentWorkerClaudeOption:
    'claude (exécute de vraies commandes — exécution de code arbitraire et modification de fichiers pour les expéditeurs autorisés de ce salon)',
  addAgentClaudeWarning:
    'AVERTISSEMENT — --worker claude exécute la CLI claude avec --permission-mode acceptEdits à chaque message déclencheur provenant d’un expéditeur autorisé. Cela revient à une exécution de code arbitraire et à la modification de fichiers sur cette machine. Ne l’activez que pour un salon et des expéditeurs auxquels vous faites confiance.',
  addAgentMintInvite: 'Émettre l’invitation d’agent',
  addAgentMinting: 'Émission…',
  addAgentResultIntro:
    'Exécutez ceci sur la machine de l’agent pour le faire entrer dans le salon. Le daemon n’offre aucun appel «\u202fspawn agent\u202f» — c’est voulu\u00a0: un humain doit copier et exécuter cette commande lui-même.',
  addAgentLaunchCommandLabel: 'Commande de lancement de l’agent',
  addAgentCopyCommand: 'Copier la commande',
  addAgentGuidance:
    'Le runner se trouve dans le dépôt — clonez-le et exécutez cette commande depuis la copie locale (pas besoin de {npm}\u202f; Node 22+ requis). Vous avez installé {jeliyad} via brew ou un script plutôt que de le compiler\u202f? Préfixez la commande par {prefix} pour que le runner le trouve. Guide complet\u00a0: {guide}.',
  addAgentTicketOnly: 'Ticket seul (si vous assemblez la commande vous-même)\u00a0:',
  addAgentCopyTicket: 'Copier le ticket',
  addAgentNoDialableAddr:
    'Ce daemon n’a signalé aucune adresse joignable directement — l’agent peut se connecter via relay ou discovery.',
  addAgentNewInvite: 'Nouvelle invitation',

  // -- invite -----------------------------------------------------------------
  inviteExpiry1h: '1 heure',
  inviteExpiry24h: '24 heures',
  inviteExpiry7d: '7 jours',
  inviteExpiryNever: 'Sans expiration',
  inviteLifecycleJoined: 'A rejoint',
  inviteLifecycleExpired: 'Expiré',
  inviteLifecycleWaiting: 'En attente',
  inviteLifecycleJoinedCopy:
    'Elle a rejoint le salon — la liste des membres confirme une adhésion active.',
  inviteLifecycleExpiredCopy:
    'Ce ticket a expiré avant qu’elle ne rejoigne le salon. Envoyez-en un nouveau ci-dessous.',
  inviteLifecycleWaitingCopy:
    'En attente qu’elle rejoigne le salon. Cela se met à jour tout seul quand la liste des membres change.',
  inviteExpiryErrorTitle: 'Cette expiration n’est pas valide',
  inviteExpiryErrorMessage: 'L’expiration doit être un nombre positif de secondes.',
  inviteExpiryErrorHint: 'Laissez le champ vide ou utilisez une valeur comme 3600.',
  inviteShareTitle: 'Invitation à un salon Jeliya',
  inviteTitle: 'Inviter dans le salon',
  inviteReadyToSend: 'Prêt à envoyer.',
  inviteReadyToSendCopy:
    'Restez dans ce salon jusqu’à ce que la personne invitée l’ait rejoint. Si elle voit encore «\u202fImpossible de joindre l’invitant\u202f», copiez une nouvelle invitation et réessayez.',
  inviteNoDialableAddress: 'Aucune adresse joignable signalée pour le moment.',
  inviteNoDialableAddressCopy:
    'Gardez ce salon ouvert. La personne qui rejoint peut encore se connecter via discovery ou relay, mais une adresse de salon récente est plus fiable.',
  inviteCombinedCopy:
    'Envoyez ce bloc unique à la personne invitée — il réunit le ticket et votre adresse joignable. Elle le colle dans «\u202fRejoindre avec un ticket\u202f» et l’adresse se remplit automatiquement.',
  inviteTicketOnlyCopy:
    'Envoyez ce ticket à la personne invitée. Elle l’utilise pour rejoindre le salon (room.join).',
  inviteCombinedInviteLabel: 'Invitation combinée (ticket et adresse du pair)',
  inviteInviteTicketLabel: 'Ticket d’invitation',
  inviteCopyInvite: 'Copier l’invitation',
  inviteCopyTicket: 'Copier le ticket',
  inviteShareInvite: 'Partager l’invitation',
  inviteShareTicket: 'Partager le ticket',
  inviteQrLabel:
    'QR code de l’invitation au salon — scannez-le sur un autre appareil pour rejoindre',
  inviteQrCombinedCaption: 'Scannez pour rejoindre — c’est la même invitation que ci-dessus.',
  inviteQrTicketCaption: 'Scannez pour importer ce ticket sur un autre appareil.',
  inviteSeparatelySummary: 'Envoyer le ticket et l’adresse séparément',
  inviteCopyAddress: 'Copier l’adresse',
  inviteNoDialableAddressNote:
    'Ce daemon n’a pas signalé d’adresse joignable — la personne qui rejoint peut se connecter via relay ou discovery.',
  inviteGenerating: 'Génération…',
  inviteAgain: 'Inviter à nouveau',
  inviteNewInvite: 'Nouvelle invitation',
  inviteAlreadyInvited:
    'Vous avez déjà invité cette identité et elle n’a pas encore rejoint le salon. Envoyez une nouvelle invitation ci-dessous.',
  inviteIntro:
    'Les tickets sont liés à une seule identité. Demandez son identifiant d’identité à la personne invitée — il apparaît sur son écran d’accueil et au bas de sa barre latérale, avec un bouton de copie.',
  inviteRoomOpenForInviting: 'Ce salon est ouvert aux invitations.',
  inviteRoomOpenForInvitingCopy:
    'Gardez-le ouvert jusqu’à ce que la personne invitée ait fini de rejoindre le salon. Jeliya ne peut effectuer l’amorçage que tant qu’un propriétaire est joignable.',
  inviteInviteeIdentityId: 'Identifiant d’identité de la personne invitée',
  inviteInviteePlaceholder: 'Identifiant d’identité de 64 caractères hexadécimaux',
  inviteIdentityInvalid:
    'Ce n’est pas un identifiant d’identité valide — il doit comporter exactement 64 caractères hexadécimaux.',
  inviteIdentityHint:
    'Collez l’identifiant d’identité de 64 caractères hexadécimaux de la personne invitée, affiché sur son écran d’accueil et au bas de sa barre latérale.',
  inviteRoleLabel: 'Rôle',
  inviteRoleMemberConsequence:
    '{role} — une personne dans le salon qui lit, publie et partage des fichiers. Aucune exécution de commande.',
  inviteRoleAgentConsequence:
    '{role} — un participant automatisé qui peut agir sur les messages autorisés de ce salon.',
  inviteAgentWarning:
    'AVERTISSEMENT — une invitation d’agent autorise un participant automatisé. Générer le ticket ne démarre rien\u00a0: un humain doit exécuter l’agent sur sa propre machine, où il peut exécuter les commandes autorisées de ce salon — exécution de code arbitraire et modification de fichiers sur cette machine. N’invitez un agent que pour un salon et des expéditeurs auxquels vous faites confiance.',
  inviteTicketExpiryLabel: 'Expiration du ticket',
  inviteAdvancedExpiry: 'Avancé / expiration personnalisée',
  inviteCustomExpiryLabel: 'Expiration personnalisée en secondes',
  inviteCustomExpiryOverride: '(remplace le préréglage ci-dessus)',
  inviteSendFresh: 'Envoyer une nouvelle invitation',
  inviteGenerateTicket: 'Générer le ticket',

  // -- room header and inspector ---------------------------------------------
  roomNavLabel: 'Outils du salon',
  roomBackToActivity: 'Retour à l’activité',
  roomCloseInspector: 'Fermer l’inspecteur',
  roomInformation: 'Informations du salon',
  roomInfoRoom: 'Salon',
  roomInfoSession: 'Session',
  roomInfoAgents: 'Agents',
  roomInfoInvites: 'Invitations',
  roomLoadingMembers: 'Chargement des membres…',
  commonMemberCount: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} membre` : `${formatted} membres`,
  roomHeaderAgentCount: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} agent` : `${formatted} agents`,
  roomHeaderInvitesPending: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} invitation en attente` : `${formatted} invitations en attente`,
  roomHeaderNoPeersConnected: 'Aucun pair connecté',
  roomHeaderPeerToPeer: 'Pair-à-pair',
  roomHeaderRelayOnly: 'Relais uniquement',
  roomHeaderPeerConnected: 'Connecté',
  roomHeaderPeerConnecting: 'Connexion…',
  roomHeaderShareFile: 'Partager un fichier',
  roomHeaderOpenPipe: 'Ouvrir un pipe',
  roomHeaderInvite: 'Inviter',
  roomHeaderPeerConnections: 'Connexions aux pairs',
  roomHeaderPeerStateConnected: 'connecté',
  roomHeaderPeerStateConnecting: 'connexion…',
  roomHeaderPeerStateOffline: 'hors ligne',

  // -- People, Agents, Files, and Pipes inspector -----------------------------
  panelMembersEmpty: 'Aucun membre n’est encore synchronisé pour ce salon.',
  panelRoomMemberCount: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} membre du salon` : `${formatted} membres du salon`,
  panelRosterCopy:
    'Liste des membres issue de l’historique signé du salon. Les statuts reflètent les événements d’adhésion, et non la joignabilité en temps réel des pairs.',
  panelRoomRoster: 'Liste des membres du salon',
  panelInvitedCount: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} invité` : `${formatted} invités`,
  panelThisDevice: 'cet appareil',
  panelLeave: 'Quitter',
  panelOwnerStays: 'Le propriétaire reste',
  panelOwnerStaysTitle:
    'Les propriétaires ne peuvent pas quitter le salon tant que le transfert de propriété n’existe pas.',
  panelAgentsEmpty:
    'Aucun membre agent dans ce salon pour l’instant. Invitez-en un avec le rôle «\u202fagent\u202f».',
  panelNoStatusPostedYet: 'Aucun statut publié pour l’instant',
  panelAgentStatusFooter: (status) => `statut\u00a0: ${status}`,
  panelKindBinary: 'binaire',
  panelKindText: 'texte',
  panelKindFile: 'fichier',
  panelFilesHeroEmptyDetail:
    'Partagez un chemin lisible et les pairs pourront en récupérer une copie vérifiée en P2P.',
  panelFilesHeroDetail: (totalBytes, availableCount, formatted = String(availableCount)) =>
    availableCount === 0 || availableCount === 1
      ? `${totalBytes} dans le salon · ${formatted} récupérable ici`
      : `${totalBytes} dans le salon · ${formatted} récupérables ici`,
  panelNFetched: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} récupéré` : `${formatted} récupérés`,
  panelServedByYou: (n, formatted = String(n)) => `Vous en diffusez ${formatted}`,
  panelNoSharedFilesYet: 'Aucun fichier partagé pour l’instant',
  panelSharedFileCount: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} fichier partagé` : `${formatted} fichiers partagés`,
  panelFileAvailabilityLabel: 'Disponibilité des fichiers',
  panelFetchableNow: 'Récupérables maintenant',
  panelFetchableNowValue: (available, total, availableText = String(available), totalText = String(total)) =>
    `${availableText}/${totalText}`,
  panelProviderDevices: 'Appareils fournisseurs',
  panelFilesShareToggle: 'Partager un fichier',
  panelFilesShareToggleClose: 'Fermer',
  panelShareCardTitle: 'Choisir un fichier à partager',
  panelShareCardHelp:
    'Sélectionnez un fichier local. Jeliya le téléverse vers ce daemon, l’importe dans le stockage de blobs du salon et le vérifie par empreinte de contenu.',
  panelHashCheckedBadge: 'empreinte vérifiée',
  panelHashCheckedBadgeLabel: 'Vérifié par empreinte de contenu',
  panelChooseFileToShare: 'Choisir un fichier à partager',
  panelNoFileSelectedYet: 'Aucun fichier sélectionné pour l’instant.',
  panelClearSelectedFile: 'Effacer',
  panelShare: 'Partager',
  panelSharing: 'Partage…',
  panelAdvancedPathSummary: 'Avancé\u00a0: collez un chemin lisible par le daemon',
  panelPathPlaceholder: '/chemin/vers/rapport.pdf',
  panelPathFieldLabel: 'Chemin du fichier à partager',
  panelPathHint:
    'À utiliser uniquement pour les fichiers déjà présents dans le répertoire de données du daemon.',
  panelSharedInThisRoom: 'Partagés dans ce salon',
  panelAllFetchable: 'Tous récupérables',
  panelAwaitingProvider: (n, formatted = String(n)) => `${formatted} en attente d’un fournisseur`,
  panelHealthServingToPeers: 'Diffusé aux pairs',
  panelHealthFetchedLocally: 'Récupéré localement',
  panelHealthSecurityCheckFailed: 'Échec du contrôle de sécurité',
  panelHealthFetchFailed: 'Échec de la récupération',
  panelHealthReadyToFetch: 'Récupérable',
  panelNProviders: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} fournisseur` : `${formatted} fournisseurs`,
  pipeStateConnected: 'Connecté',
  pipeStateOpen: 'Ouvert',
  pipeStateClosed: 'Fermé',
  panelExposeTitle: 'Exposer un pipe',
  panelExposeCopy: 'Redirigez un port local vers un seul pair autorisé.',
  panelTargetFieldLabel: 'Cible locale (host:port)',
  panelAuthorizedPeerLabel: 'Pair autorisé',
  panelNoOtherMembers: 'aucun autre membre',
  panelPeerChoice: (name, role) => `${name} (${role})`,
  panelExpose: 'Exposer',
  panelExposing: 'Exposition…',
  panelPipesEmpty:
    'Aucun pipe pour l’instant — exposez ci-dessus un port local à un seul pair autorisé.',
  panelPipeMeta: 'par {openedBy} · autorisé\u00a0: {authorized}',
  panelConnect: 'Se connecter',
  panelConnecting: 'Connexion…',
  panelOpenPreview: 'Ouvrir l’aperçu ↗',
  panelClosePipe: 'Fermer',
  panelClosingPipe: 'Fermeture…',
  panelInspectorLabel: (tool) => `Inspecteur «\u202f${tool}\u202f»`,

  // -- timeline ---------------------------------------------------------------
  timelineRoomTimeline: 'Fil du salon',
  timelineEmptyState: 'Aucun événement pour l’instant — écrivez quelque chose ci-dessous.',
  timelineAgentChip: 'agent',
  timelineStatusFallback: 'statut',
  timelineFileSharedMeta: '{sender} {role} a partagé un fichier · {time}',
  timelinePipeOpenedMeta: '{sender} {role} a ouvert un pipe · {time}',
  timelineFileMeta: '{bytes} · {ext}',
  timelineOpenInPipes: 'Ouvrir dans Pipes',
  timelineOpenInFiles: 'Ouvrir dans Fichiers',
  timelineAuthorizedPeer: 'pair autorisé\u00a0: {peer}',
  timelineSyslineRoomCreated: '{sender} a créé le salon · {time}',
  timelineSyslineInvited: '{sender} a invité {invitee} en tant que {role} · {time}',
  timelineSyslineInvitedNoRole: '{sender} a invité {invitee} · {time}',
  timelineSyslineJoined: '{who} a rejoint le salon en tant que {role} · {time}',
  timelineSyslineLeft: '{who} a quitté le salon · {time}',
  timelineSyslinePipeClosed: '{sender} a fermé le pipe {target} · {time}',
  timelineSomeone: 'quelqu’un',
  timelinePendingSending: 'Envoi…',
  timelinePendingSyncing: 'Envoyé localement, synchronisation…',
  timelinePendingFailed: 'Échec de l’envoi',
  timelineRetryMessage: 'Réessayer d’envoyer le message',
  timelineRetryMessageAt: (time) => `Réessayer d’envoyer votre message de ${time}`,
  timelineNewMessages: (n, formatted = String(n)) =>
    n === 0 || n === 1 ? `${formatted} nouveau message` : `${formatted} nouveaux messages`,
  timelineNewActivity: (n) => `${n} nouvelle activité`,
  timelineRunEvidence: (count, span) => `${count} mises à jour · ${span}`,
  timelineRunShow: (count) => `Afficher ${count} mises à jour`,
  timelineRunHide: 'Masquer',
  timelineFilterActivity: 'Filtrer l’activité',
  timelineFilterConversation: 'Conversation',
  timelineFilterAgentRuns: 'Exécutions d’agent',
  timelineFilterMembership: 'Membres',
  timelineFilterFiles: 'Fichiers',
  timelineFilterPipes: 'Pipes',
  timelineNoActivityMatches: 'Aucune activité ne correspond à ces filtres.',

  // -- modals ------------------------------------------------------------------
  modalJoinCopy:
    'Collez l’invitation que vous avez reçue. Une invitation combinée ({combined}) renseigne automatiquement ' +
    'l’adresse du pair.',
  modalTicketLabel: 'Ticket',
  modalTicketPlaceholder: 'roomtkt1… ou roomtkt1…#<endpoint_id>@host:port',
  modalPeerAddrLabel: 'Adresse du pair',
  modalJoinSubmit: 'Rejoindre le salon',
  modalJoining: 'Connexion…',
  modalJoinAttempt: (attempt, max, attemptText = String(attempt), maxText = String(max)) =>
    `Tentative ${attemptText}/${maxText}`,
  modalCreateTitle: 'Créer un salon',
  modalRoomNameLabel: 'Nom du salon',
  modalRoomNamePlaceholder: 'Construire le MVP Iroh Rooms',
  modalCreating: 'Création…',
  modalCreateHomonymWarning:
    'Un salon portant ce nom existe déjà sur cet appareil — celui-ci aura son propre identifiant.',
  modalLeaveTitle: 'Quitter le salon',
  modalLeaveCopy:
    'Quitter {room} {id} publie une annonce de départ signée. Ce n’est pas la même chose que fermer la ' +
    'session locale\u202f; il vous faudra une nouvelle invitation pour rejoindre le salon à nouveau.',
  modalLeaveSubmit: 'Quitter le salon',
  modalLeaving: 'Départ…',
  modalRenameTitle: 'Nommer ce pair',
  modalRenameCopy: 'Alias local uniquement — les noms ne quittent jamais cette machine.',
  modalRenameIdentityLabel: 'Identité\u00a0:',
  modalRenameAliasLabel: 'Alias',
  modalRenameAliasPlaceholder: 'ex. Awa D.',
  modalRenameClearAlias: 'Supprimer l’alias',

  // -- onboarding -------------------------------------------------------------
  onboardingTagline: 'Jeliya — l’art du djéli, gardien de la mémoire vraie.',
  onboardingIdentityTitle: 'Créez votre identité',
  onboardingIdentityCopy1:
    'Une paire de clés générée et conservée par votre daemon local. Pas de compte, pas de serveur — la clé privée ne quitte jamais cette machine.',
  onboardingIdentityCopy2:
    'Il n’existe ni réinitialisation de mot de passe ni récupération — si vous perdez cet appareil ou son dossier de données, cette identité est définitivement perdue.',
  onboardingCreateIdentity: 'Créer une identité',
  onboardingCreatingIdentity: 'Création…',
  onboardingYourIdentityId: 'Votre identifiant d’identité',
  onboardingIdentityCardCopy1:
    'Vous attendez une invitation à un salon\u202f? Envoyez d’abord cet ID à la personne qui vous invite — les tickets y sont liés.',
  onboardingIdentityCardCopy2:
    'Les pairs apparaissent d’abord sous un ID hexadécimal du même type — cliquez sur le nom d’un pair dans un salon pour lui attribuer un surnom local (visible uniquement par vous).',
  onboardingCreateRoomCopy: 'Ouvrez un espace et invitez des personnes ou des agents avec des tickets.',
  onboardingJoinFinding:
    'Recherche de la personne qui vous invite et synchronisation de l’invitation au salon…',
  onboardingJoinRetryingAttempt: (attempt, max, attemptText = String(attempt), maxText = String(max)) =>
    `Nouvelle tentative pour rejoindre le salon (${attemptText}/${maxText})…`,
  onboardingJoinRetryWait: (seconds, formatted = String(seconds)) =>
    `Le premier chemin n’a pas répondu. Nouvelle tentative dans ${formatted} s…`,

  // -- composer ---------------------------------------------------------------
  composerMessagePlaceholder: (roomName) => `Écrire dans ${roomName}`,
  composerSendMessage: 'Envoyer le message',
  composerShareAFile: 'Partager un fichier',
  composerHint: 'Entrée pour envoyer · Maj+Entrée pour aller à la ligne · ⎘ pour partager un fichier',
  composerSharingFile: 'Partage du fichier…',

  // -- formatting vocabulary ---------------------------------------------------
  formatToday: 'Aujourd’hui',
  formatYesterday: 'Hier',
  // Octets, not bytes (decision 7). Unit WORDS follow the text locale; the
  // number inside them already came from the formatting locale.
  formatBytesB: (n) => `${n} o`,
  formatBytesKb: (n) => `${n} Ko`,
  formatBytesMb: (n) => `${n} Mo`,
  formatBytesGb: (n) => `${n} Go`,
  formatPercent: (n) => `${n}\u202f%`,
  formatJustNow: 'à l’instant',
  formatMinutesAgo: (n) => `il y a ${n} min`,
  formatHoursAgo: (n) => `il y a ${n} h`,
  formatDaysAgo: (n) => `il y a ${n} j`,
};
