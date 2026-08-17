//! Pure, host-testable view-model folds for the #180 product surfaces (§3).
//!
//! Nothing here renders (no Dioxus) and nothing touches a platform (`web-sys`,
//! `cfg`): each module folds typed `jeliya_api` reads into a view model the thin
//! RSX components render, so the People/Agents/Fleet/Settings correctness lives
//! in `cargo test -p jeliya-ui` and every target reuses it unchanged.

pub mod agents;
pub mod alias;
pub mod capability;
pub mod fleet;
pub mod invites;
pub mod load;
pub mod poll;
pub mod roster;
