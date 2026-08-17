//! French — implements the same [`Catalog`] as [`super::En`], so a missing key
//! is a compile error (§4-D1). The node gate is defence in depth for what the
//! compiler cannot see: a value left in English, and the French typography
//! contract (§5.4) — U+202F before `%`, U+2019 for the apostrophe, U+2026 for
//! the ellipsis. The narrow no-break space is written `\u{202f}` so it is
//! visible in review; every other typographic character is written as its glyph
//! (`…`, `’`, `—`), exactly as the French copy renders.

use super::{Catalog, PluralCategory};

/// The French catalog. A zero-sized dispatch target; [`super::catalog_for`]
/// hands out `&'static Fr`.
pub struct Fr;

impl Catalog for Fr {
    fn locale_tag(&self) -> &'static str {
        "fr"
    }
    fn app_name(&self) -> &'static str {
        // The catalog gate (text-based) requires a string literal here; the
        // `tokens::BRAND` equality is enforced by a test (see `tokens.rs`), so a
        // brand change that does not update both fails CI.
        "Jeliya"
    }

    fn boot_connecting(&self) -> &'static str {
        "connexion au démon local…"
    }
    fn boot_stopping(&self) -> &'static str {
        "arrêt — vidange du travail accepté…"
    }
    fn boot_stopped(&self) -> &'static str {
        "arrêté"
    }
    fn boot_failed(&self) -> &'static str {
        "le client a échoué et ne réessaiera pas"
    }

    fn rooms_heading(&self) -> &'static str {
        "Salons"
    }
    fn rooms_empty(&self) -> &'static str {
        "Aucun salon"
    }
    fn rooms_loading(&self) -> &'static str {
        "Chargement des salons…"
    }
    fn room_untitled(&self) -> &'static str {
        "Salon sans titre"
    }
    fn center_choose_room(&self) -> &'static str {
        "Choisissez un salon"
    }

    fn skip_to_rooms(&self) -> &'static str {
        "Aller aux salons"
    }
    fn skip_to_content(&self) -> &'static str {
        "Aller au contenu principal"
    }

    fn diagnostics_open(&self) -> &'static str {
        "Diagnostics"
    }
    fn diagnostics_title(&self) -> &'static str {
        "Diagnostics"
    }
    fn diagnostics_close(&self) -> &'static str {
        "Fermer"
    }
    fn diagnostics_lifecycle_label(&self) -> &'static str {
        "État du client"
    }
    fn diagnostics_detail_label(&self) -> &'static str {
        "Détail de la dernière erreur"
    }
    fn diagnostics_no_detail(&self) -> &'static str {
        "Aucune erreur enregistrée."
    }

    fn err_room_list_title(&self) -> &'static str {
        "Échec du chargement des salons"
    }
    fn err_room_list_message(&self) -> &'static str {
        "La liste des salons ne s’est pas chargée. Jeliya réessaiera au retour de la connexion."
    }
    fn err_room_list_terminal_message(&self) -> &'static str {
        "La liste des salons n’a pas pu se charger. Ouvrez Diagnostics pour en savoir plus."
    }
    fn err_unknown_title(&self) -> &'static str {
        "Une erreur est survenue"
    }
    fn err_unknown_message(&self) -> &'static str {
        "Une erreur inattendue s’est produite. Voir Diagnostics pour les détails."
    }

    fn status_connecting(&self) -> &'static str {
        "Connexion"
    }
    fn status_ready(&self) -> &'static str {
        "Connecté"
    }
    fn status_interrupted(&self) -> &'static str {
        "Reconnexion"
    }
    fn status_stopping(&self) -> &'static str {
        "Arrêt"
    }
    fn status_stopped(&self) -> &'static str {
        "Arrêté"
    }
    fn status_failed(&self) -> &'static str {
        "Déconnecté"
    }

    fn conn_announcement(&self, status: &str) -> String {
        // French: a U+00A0 (no-break space) precedes the colon.
        format!("État de la connexion\u{00a0}: {status}")
    }

    fn wire_role_authority(&self) -> &'static str {
        "Autorité"
    }
    fn wire_role_member(&self) -> &'static str {
        "Membre"
    }
    fn wire_status_active(&self) -> &'static str {
        "Membre"
    }
    fn wire_status_invited(&self) -> &'static str {
        "Invité"
    }
    fn wire_status_left(&self) -> &'static str {
        "Parti"
    }
    fn wire_status_removed(&self) -> &'static str {
        "Retiré"
    }
    fn wire_status_unknown(&self) -> &'static str {
        "Inconnu"
    }
    fn wire_path_direct(&self) -> &'static str {
        // Tier-2 protocol token: kept VERBATIM as the daemon reports it
        // (docs/glossary-fr.md decision — `direct`/`relay` are never translated).
        "direct"
    }
    fn wire_path_relay(&self) -> &'static str {
        "relay"
    }

    fn format_percent(&self, n: &str) -> String {
        format!("{n}\u{202f}%")
    }
    fn format_bytes_b(&self, n: &str) -> String {
        format!("{n} o")
    }
    fn format_bytes_kb(&self, n: &str) -> String {
        format!("{n} Ko")
    }
    fn format_bytes_mb(&self, n: &str) -> String {
        format!("{n} Mo")
    }
    fn format_bytes_gb(&self, n: &str) -> String {
        format!("{n} Go")
    }
    fn format_today(&self) -> &'static str {
        "Aujourd’hui"
    }
    fn format_yesterday(&self) -> &'static str {
        "Hier"
    }
    fn month_name(&self, month: u32) -> Option<&'static str> {
        match month {
            1 => Some("janvier"),
            2 => Some("février"),
            3 => Some("mars"),
            4 => Some("avril"),
            5 => Some("mai"),
            6 => Some("juin"),
            7 => Some("juillet"),
            8 => Some("août"),
            9 => Some("septembre"),
            10 => Some("octobre"),
            11 => Some("novembre"),
            12 => Some("décembre"),
            // Out-of-range guard: unreachable for a validated 1..=12 month (the
            // domain of the Gregorian calendar). `None`, not a panic — a total
            // function keeps a stray index from crashing a render.
            _ => None,
        }
    }

    fn format_just_now(&self) -> &'static str {
        "à l’instant"
    }
    fn format_minutes_ago(&self, n: &str) -> String {
        format!("il y a {n} min")
    }
    fn format_hours_ago(&self, n: &str) -> String {
        format!("il y a {n} h")
    }
    fn format_days_ago(&self, n: &str) -> String {
        format!("il y a {n} j")
    }

    fn client_status(&self, lifecycle: &str) -> String {
        format!(
            "client {dot} {lifecycle}",
            dot = crate::l10n::tokens::MIDDLE_DOT
        )
    }

    fn nav_global_label(&self) -> &'static str {
        "Principal"
    }
    fn dest_rooms(&self) -> &'static str {
        "Salons"
    }
    fn dest_fleet(&self) -> &'static str {
        "Flotte d’agents"
    }
    fn dest_settings(&self) -> &'static str {
        "Paramètres"
    }

    fn onboarding_identity_title(&self) -> &'static str {
        "Créez votre identité"
    }
    fn onboarding_identity_body(&self) -> &'static str {
        "Votre identité est une paire de clés sur cet appareil. Il n’y a ni compte ni mot de passe à récupérer."
    }
    fn onboarding_create_identity(&self) -> &'static str {
        "Créer l’identité"
    }
    fn identity_id_label(&self) -> &'static str {
        "Votre identité"
    }
    fn identity_copy(&self) -> &'static str {
        "Copier"
    }
    fn identity_unrecoverable(&self) -> &'static str {
        "Ceci est votre identité pair-à-pair permanente. Elle est irrécupérable en cas de perte."
    }

    fn onboarding_rooms_title(&self) -> &'static str {
        "Créez ou rejoignez un salon"
    }
    fn onboarding_create_room(&self) -> &'static str {
        "Créer un salon"
    }
    fn room_name_label(&self) -> &'static str {
        "Nom du salon"
    }
    fn onboarding_join_room(&self) -> &'static str {
        "Rejoindre avec un ticket"
    }
    fn ticket_label(&self) -> &'static str {
        "Ticket d’invitation"
    }
    fn ticket_help(&self) -> &'static str {
        "Collez un ticket d’invitation qu’un membre du salon a partagé avec vous."
    }

    fn self_label_label(&self) -> &'static str {
        "Votre nom"
    }
    fn self_label_help(&self) -> &'static str {
        "Affiché uniquement sur cet appareil. Il n’est jamais envoyé à personne."
    }

    fn settings_heading(&self) -> &'static str {
        "Paramètres"
    }
    fn settings_identity_heading(&self) -> &'static str {
        "Identité"
    }
    fn settings_language_heading(&self) -> &'static str {
        "Langue"
    }
    fn settings_text_locale_label(&self) -> &'static str {
        "Langue de l’interface"
    }
    fn settings_formatting_locale_label(&self) -> &'static str {
        "Format des nombres et des dates"
    }
    fn settings_locale_follow_system(&self) -> &'static str {
        "Suivre le système"
    }
    fn settings_session_only_note(&self) -> &'static str {
        "S’applique à cette session — non enregistré sur ce navigateur."
    }

    fn fleet_heading(&self) -> &'static str {
        "Flotte d’agents"
    }
    fn fleet_loading(&self) -> &'static str {
        "La flotte d’agents n’est pas encore disponible."
    }

    fn room_nav_label(&self) -> &'static str {
        "Salon"
    }
    fn room_dest_activity(&self) -> &'static str {
        "Activité"
    }
    fn room_dest_people(&self) -> &'static str {
        "Personnes"
    }
    fn room_dest_agents(&self) -> &'static str {
        // `agents` is a never-translate lexicon term (docs/glossary-fr.md);
        // identical to English on purpose, as the retiring React catalog had it.
        "Agents"
    }
    fn room_dest_files(&self) -> &'static str {
        "Fichiers"
    }
    fn room_dest_pipes(&self) -> &'static str {
        // Coined protocol term, kept verbatim — the retiring React catalog
        // rendered `Pipes` identically in French too.
        "Pipes"
    }
    fn room_dest_skeleton(&self) -> &'static str {
        "Rien à afficher ici pour l’instant."
    }
    fn room_unavailable(&self) -> &'static str {
        "Ce salon n’est pas disponible sur cet appareil. Revenez aux salons."
    }

    fn recovery_title(&self) -> &'static str {
        "Préférences locales réinitialisées"
    }
    fn recovery_body(&self) -> &'static str {
        "Certaines préférences locales n’ont pas pu être lues et ont été réinitialisées."
    }
    fn recovery_reset_action(&self) -> &'static str {
        "Réinitialiser les préférences locales"
    }

    fn err_onboarding_identity(&self) -> &'static str {
        "Échec de la création de votre identité"
    }
    fn err_onboarding_identity_body(&self) -> &'static str {
        "Le démon n’a pas répondu. Vérifiez votre connexion et réessayez."
    }

    fn err_onboarding_room_create(&self) -> &'static str {
        "Échec de la création du salon"
    }
    fn err_onboarding_room_create_body(&self) -> &'static str {
        "Le démon n’a pas répondu. Vérifiez votre connexion et réessayez."
    }

    fn rooms_count(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} salon"),
            PluralCategory::Other => format!("{count_display} salons"),
        }
    }
}
