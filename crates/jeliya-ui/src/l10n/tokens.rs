//! Never-translate constants (i18n.md rule 1; spec §5.1).
//!
//! Glyphs, the brand, shell/wire examples, and language endonyms are **outside**
//! the [`Catalog`](super::Catalog) by design: they are the same in every
//! language, so routing them through a translated message would invite a
//! translator to "fix" a value that must not change and would make the French
//! catalog claim to translate the brand. The literal-copy gate treats this
//! module as exempt for exactly that reason (the React `EXEMPT_FILES` records
//! the same decision for `ui/src/l10n/tokens.ts`).

/// The product name. Tier 3 of the never-translate glossary.
pub const BRAND: &str = "Jeliya";

/// The middle dot used between two facts in a status line (`client · Ready`).
/// A glyph, not copy — it carries no words to translate.
pub const MIDDLE_DOT: &str = "·";

/// The endonym of each supported language, for a (later) language picker. An
/// endonym is written in its own language, so it is never translated.
pub const ENDONYM_EN: &str = "English";
/// French endonym.
pub const ENDONYM_FR: &str = "Français";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{Catalog, En, Fr};

    /// These never-translate tokens are the AUTHORITATIVE spellings, so the
    /// catalogs must agree with them. `client_status` consumes `MIDDLE_DOT`
    /// directly; `app_name` keeps a string literal (the text-based catalog gate
    /// requires one), so this test is what makes `BRAND` load-bearing — a brand or
    /// shell-token change that updates only one side fails here.
    #[test]
    fn the_catalogs_agree_with_the_never_translate_tokens() {
        assert_eq!(En.app_name(), BRAND, "EN brand must be the canonical token");
        assert_eq!(Fr.app_name(), BRAND, "FR brand must be the canonical token");
        for status in [En.client_status("Ready"), Fr.client_status("Ready")] {
            assert!(
                status.contains(MIDDLE_DOT),
                "the status line must use the canonical middle dot: {status}"
            );
        }
    }
}
