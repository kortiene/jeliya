//! Pure scroll math for the Activity pane (#179 §7, D6).
//!
//! The React `Timeline` scroll machine, re-expressed as pure transitions fed by
//! Dioxus-measured numbers: [`ScrollModel`] holds the state the React refs held
//! (`stick_to_bottom`, the last reading offset, a pending restore, a reload
//! baseline, the new-item count) and decides a [`ScrollAction`] the component
//! applies through the mounted element handle. **No DOM here** — the component
//! measures `scroll_offset`/`scroll_size`/`client_rect` and calls `scroll_to`;
//! this module is just the arithmetic (D6), so it is host-tested exhaustively.
//!
//! The wording of the "new" affordance (messages-only vs. mixed) is *not* here:
//! it depends on what the trailing items are, which the component derives from
//! the timeline. This module counts; the component words.

/// A measurement snapshot from the scroller element (all in CSS pixels).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Measure {
    /// The current scroll offset from the top.
    pub scroll_top: f64,
    /// The full scrollable content height.
    pub scroll_height: f64,
    /// The visible viewport height.
    pub client_height: f64,
}

/// A deliberately-preserved reading position, saved across a room switch or a
/// backlog reload (§7 `save_view`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SavedView {
    /// The scroll offset to restore.
    pub scroll_top: f64,
    /// The item count seen at save time — the baseline the new-item delta is
    /// derived from once the backlog is reloaded.
    pub item_count: usize,
}

/// The scroll adjustment the component should apply. `None` leaves the scroller
/// where the user put it (a user scroll is never fought).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScrollAction {
    /// Leave the scroll position unchanged.
    None,
    /// Jump to the newest content (stick-to-bottom).
    ToBottom,
    /// Restore a specific offset (a saved reading position or a reveal fixup).
    ToOffset(f64),
}

/// The distance (px) from the bottom within which the view is considered "at the
/// bottom" and sticks to new content — the React 140px threshold.
pub const BOTTOM_THRESHOLD: f64 = 140.0;

/// The pure scroll state machine (§7).
#[derive(Clone, PartialEq, Debug)]
pub struct ScrollModel {
    /// Whether the view follows new content at the bottom. Public so the
    /// component can style the "jump to newest" affordance honestly.
    pub stick_to_bottom: bool,
    /// The count of new items that arrived while scrolled up — the "N new"
    /// number. Cleared on reaching the bottom.
    pub new_item_count: usize,
    last_scroll_top: f64,
    restore_pending: Option<SavedView>,
    /// A wholesale backlog reload (a reconnect re-baseline) is in flight or just
    /// completed; its items must not be announced as new activity.
    reload_baseline: bool,
    item_count: usize,
}

impl Default for ScrollModel {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollModel {
    /// A fresh model: a room opens stuck to its newest event (§7 `save_view`
    /// "a room left at the bottom re-opens at its newest event").
    pub fn new() -> Self {
        Self {
            stick_to_bottom: true,
            new_item_count: 0,
            last_scroll_top: 0.0,
            restore_pending: None,
            reload_baseline: false,
            item_count: 0,
        }
    }

    /// A user scroll: set `stick_to_bottom` within the bottom threshold (and
    /// clear the new-item count there), else release it. Never fights the user,
    /// so it returns [`ScrollAction::None`].
    pub fn on_scroll(&mut self, m: Measure) -> ScrollAction {
        self.last_scroll_top = m.scroll_top;
        let dist_from_bottom = (m.scroll_height - m.client_height - m.scroll_top).max(0.0);
        if dist_from_bottom <= BOTTOM_THRESHOLD {
            self.stick_to_bottom = true;
            self.new_item_count = 0;
        } else {
            self.stick_to_bottom = false;
        }
        ScrollAction::None
    }

    /// The timeline's item count (and loading flag) changed. This is the single
    /// place new-item accounting happens (§7 `on_items_changed`):
    ///
    /// - While `loading` (a resync), mark a reload baseline so the reloaded
    ///   backlog is not announced as new activity, and keep the position.
    /// - A pending restore, once its saved backlog is back, restores the saved
    ///   offset and derives the new count from the saved baseline (once).
    /// - A completed wholesale reload rebases silently.
    /// - Otherwise: stick to the bottom (nothing to announce), or accumulate the
    ///   new-item delta while scrolled up.
    pub fn on_items_changed(&mut self, count: usize, loading: bool) -> ScrollAction {
        if loading {
            self.reload_baseline = true;
            self.item_count = self.item_count.min(count);
            return if self.stick_to_bottom {
                ScrollAction::ToBottom
            } else {
                ScrollAction::None
            };
        }
        if let Some(saved) = self.restore_pending {
            if count >= saved.item_count {
                self.new_item_count = count - saved.item_count;
                self.item_count = count;
                self.restore_pending = None;
                self.reload_baseline = false;
                return ScrollAction::ToOffset(saved.scroll_top);
            }
            // The saved backlog is not fully reloaded yet; wait for more.
            self.item_count = count;
            return ScrollAction::None;
        }
        let delta = count.saturating_sub(self.item_count);
        self.item_count = count;
        if self.reload_baseline {
            // A wholesale reload just completed: adopt the new backlog without
            // announcing it as new activity (§7 reload baseline).
            self.reload_baseline = false;
            return if self.stick_to_bottom {
                ScrollAction::ToBottom
            } else {
                ScrollAction::None
            };
        }
        if self.stick_to_bottom {
            self.new_item_count = 0;
            ScrollAction::ToBottom
        } else {
            self.new_item_count = self.new_item_count.saturating_add(delta);
            ScrollAction::None
        }
    }

    /// Decide a target offset on a layout change (resize, rotation, a
    /// `display:none` → visible reveal) — the single reconciliation point (§7
    /// `sync`): restore a pending saved position, stick to the bottom, or
    /// reinstate the reading spot when a reveal zeroed the measurement.
    pub fn sync(&self, m: Measure) -> ScrollAction {
        if self.restore_pending.is_some() {
            // The restore is applied by `on_items_changed` once the backlog is
            // in; `sync` must not race it to a stale offset.
            return ScrollAction::None;
        }
        if self.stick_to_bottom {
            return ScrollAction::ToBottom;
        }
        if m.scroll_height <= 0.0 || m.client_height <= 0.0 {
            // A hidden pane measures zero; reinstate the last reading offset so
            // the reveal does not jump to the top (the compact-pane case).
            return ScrollAction::ToOffset(self.last_scroll_top);
        }
        ScrollAction::None
    }

    /// Capture the deliberate reading position to survive a room switch (§7). A
    /// view left at the bottom returns `None` — it re-opens at its newest event.
    pub fn save_view(&self, m: Measure) -> Option<SavedView> {
        if self.stick_to_bottom {
            None
        } else {
            Some(SavedView {
                scroll_top: m.scroll_top,
                item_count: self.item_count,
            })
        }
    }

    /// Re-seed the model on returning to a room. `None` (nothing deliberate was
    /// saved) sticks to the bottom; `Some` schedules a one-shot restore once the
    /// backlog reloads. Either way a fresh open announces no new activity.
    pub fn restore(&mut self, saved: Option<SavedView>) {
        match saved {
            None => {
                self.stick_to_bottom = true;
                self.restore_pending = None;
            }
            Some(view) => {
                self.stick_to_bottom = false;
                self.restore_pending = Some(view);
            }
        }
        self.new_item_count = 0;
        self.reload_baseline = false;
    }

    /// The user tapped the "N new" affordance: jump to the newest content and
    /// resume sticking to the bottom.
    pub fn jump_to_new(&mut self) -> ScrollAction {
        self.stick_to_bottom = true;
        self.new_item_count = 0;
        ScrollAction::ToBottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_bottom() -> Measure {
        Measure {
            scroll_top: 900.0,
            scroll_height: 1000.0,
            client_height: 100.0,
        }
    }

    fn scrolled_up() -> Measure {
        Measure {
            scroll_top: 0.0,
            scroll_height: 1000.0,
            client_height: 100.0,
        }
    }

    #[test]
    fn a_fresh_model_sticks_to_the_bottom() {
        let m = ScrollModel::new();
        assert!(m.stick_to_bottom);
        assert_eq!(m.new_item_count, 0);
    }

    #[test]
    fn scrolling_within_the_threshold_sticks_and_away_releases() {
        let mut m = ScrollModel::new();
        // Just within 140px of the bottom.
        m.on_scroll(Measure {
            scroll_top: 780.0,
            scroll_height: 1000.0,
            client_height: 100.0,
        });
        assert!(m.stick_to_bottom);
        // Well above the threshold.
        m.on_scroll(scrolled_up());
        assert!(!m.stick_to_bottom);
    }

    #[test]
    fn new_items_while_scrolled_up_accumulate_and_reaching_bottom_clears() {
        let mut m = ScrollModel::new();
        m.on_items_changed(10, false); // establish baseline at bottom
        m.on_scroll(scrolled_up());
        assert_eq!(m.on_items_changed(13, false), ScrollAction::None);
        assert_eq!(m.new_item_count, 3);
        m.on_items_changed(15, false);
        assert_eq!(m.new_item_count, 5);
        // Scrolling back to the bottom clears the announced count.
        m.on_scroll(at_bottom());
        assert_eq!(m.new_item_count, 0);
        assert!(m.stick_to_bottom);
    }

    #[test]
    fn sticking_to_bottom_follows_new_items_without_announcing() {
        let mut m = ScrollModel::new();
        m.on_items_changed(10, false);
        assert_eq!(m.on_items_changed(12, false), ScrollAction::ToBottom);
        assert_eq!(m.new_item_count, 0);
    }

    #[test]
    fn a_wholesale_reload_is_not_announced_as_new_activity() {
        let mut m = ScrollModel::new();
        m.on_items_changed(50, false);
        m.on_scroll(scrolled_up()); // release stick
                                    // A resync starts (loading), then a full backlog reload lands.
        m.on_items_changed(0, true);
        let action = m.on_items_changed(50, false);
        assert_eq!(action, ScrollAction::None);
        assert_eq!(
            m.new_item_count, 0,
            "a reconnect reload is not new activity"
        );
        // A genuinely new item after the reload IS announced.
        m.on_items_changed(51, false);
        assert_eq!(m.new_item_count, 1);
    }

    #[test]
    fn save_and_restore_round_trip_preserves_the_reading_position() {
        let mut m = ScrollModel::new();
        m.on_items_changed(20, false);
        m.on_scroll(scrolled_up());
        let saved = m.save_view(scrolled_up());
        assert_eq!(
            saved,
            Some(SavedView {
                scroll_top: 0.0,
                item_count: 20,
            })
        );

        // A fresh model (a remount) restores the saved view once the backlog is
        // reloaded, deriving the new-item count from the saved baseline.
        let mut restored = ScrollModel::new();
        restored.restore(saved);
        assert!(!restored.stick_to_bottom);
        // Backlog only partially reloaded — wait.
        assert_eq!(restored.on_items_changed(10, false), ScrollAction::None);
        // Fully reloaded plus two new items: restore the offset, announce 2.
        assert_eq!(
            restored.on_items_changed(22, false),
            ScrollAction::ToOffset(0.0)
        );
        assert_eq!(restored.new_item_count, 2);
    }

    #[test]
    fn a_view_left_at_the_bottom_reopens_at_the_newest() {
        let mut m = ScrollModel::new();
        m.on_items_changed(20, false); // at bottom
        assert_eq!(
            m.save_view(at_bottom()),
            None,
            "no deliberate position saved"
        );
        let mut reopened = ScrollModel::new();
        reopened.restore(None);
        assert!(reopened.stick_to_bottom);
    }

    #[test]
    fn a_reveal_that_zeroes_the_measurement_reinstates_the_reading_spot() {
        let mut m = ScrollModel::new();
        m.on_items_changed(20, false);
        m.on_scroll(scrolled_up()); // last_scroll_top = 0
        m.on_scroll(Measure {
            scroll_top: 300.0,
            scroll_height: 1000.0,
            client_height: 100.0,
        });
        assert!(!m.stick_to_bottom);
        // A display:none reveal measures zero: reinstate the last offset.
        let hidden = Measure {
            scroll_top: 0.0,
            scroll_height: 0.0,
            client_height: 0.0,
        };
        assert_eq!(m.sync(hidden), ScrollAction::ToOffset(300.0));
    }

    #[test]
    fn sync_sticks_to_bottom_when_following() {
        let m = ScrollModel::new();
        assert_eq!(m.sync(at_bottom()), ScrollAction::ToBottom);
    }

    #[test]
    fn jump_to_new_returns_to_the_bottom_and_clears_the_count() {
        let mut m = ScrollModel::new();
        m.on_items_changed(10, false);
        m.on_scroll(scrolled_up());
        m.on_items_changed(14, false);
        assert_eq!(m.new_item_count, 4);
        assert_eq!(m.jump_to_new(), ScrollAction::ToBottom);
        assert_eq!(m.new_item_count, 0);
        assert!(m.stick_to_bottom);
    }

    #[test]
    fn sync_returns_none_during_a_pending_restore_so_it_does_not_race_the_one_shot() {
        // A saved reading position is waiting for the backlog to reload.  `sync`
        // must yield and let `on_items_changed` apply the restore, otherwise it
        // would jump the scroller to a stale offset.
        let mut m = ScrollModel::new();
        m.on_items_changed(20, false);
        m.on_scroll(scrolled_up());
        let saved = m.save_view(scrolled_up());
        let mut restored = ScrollModel::new();
        restored.restore(saved);
        // The backlog has not reloaded yet — sync must not fight the pending restore.
        assert_eq!(restored.sync(scrolled_up()), ScrollAction::None);
    }

    #[test]
    fn bottom_threshold_boundary_sticks_at_exactly_140px_and_releases_above() {
        // dist_from_bottom = scroll_height - client_height - scroll_top
        // Exactly 140 px from the bottom → ≤ threshold → sticks.
        let mut m = ScrollModel::new();
        let exactly_at_threshold = Measure {
            scroll_top: 760.0,
            scroll_height: 1000.0,
            client_height: 100.0, // dist = 1000 - 100 - 760 = 140.0
        };
        m.on_scroll(exactly_at_threshold);
        assert!(
            m.stick_to_bottom,
            "140px from the bottom is within the threshold"
        );

        // One pixel further from the bottom → > threshold → releases.
        m.on_scroll(Measure {
            scroll_top: 759.0,
            scroll_height: 1000.0,
            client_height: 100.0, // dist = 1000 - 100 - 759 = 141.0
        });
        assert!(
            !m.stick_to_bottom,
            "141px from the bottom exceeds the threshold"
        );
    }
}
