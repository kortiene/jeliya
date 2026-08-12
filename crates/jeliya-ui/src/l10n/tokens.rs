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
