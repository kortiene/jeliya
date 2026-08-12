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

    fn wire_role_owner(&self) -> &'static str {
        "Propriétaire"
    }
    fn wire_role_agent(&self) -> &'static str {
        "Agent"
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
        "direct"
    }
    fn wire_path_relay(&self) -> &'static str {
        "relais"
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

    fn client_status(&self, lifecycle: &str) -> String {
        format!(
            "client {dot} {lifecycle}",
            dot = crate::l10n::tokens::MIDDLE_DOT
        )
    }

    fn rooms_count(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} salon"),
            PluralCategory::Other => format!("{count_display} salons"),
        }
    }
}
