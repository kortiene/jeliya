//! The Fleet polling controller (§10, D7, AC-5): a pure `{active, visible}`
//! state machine that decides whether a poll tick is allowed and whether a
//! resume-fetch is owed.
//!
//! It owns no timer — the interval is driven by the composition's `use_future`;
//! the machine only decides, on each input change, what the loop should do. It
//! emits a poll tick **only** while the Fleet route is active AND the document
//! is visible, fetches **once immediately** on re-entering that condition
//! (resume-once), and never replays a backlog of missed ticks or polls while
//! backgrounded/off-destination.

/// The pure poll controller. `active` = the route is `Route::Fleet`; `visible`
/// = the document is visible (folded from the `Lifecycle` bus). `was_running`
/// records whether the previous evaluation was in the running condition, so a
/// re-entry is distinguishable from a steady tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FleetPoll {
    active: bool,
    visible: bool,
    was_running: bool,
}

/// What the poll loop should do after an input change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PollAction {
    /// Not running: stop the loop (and clear the resume-owed latch).
    Stop,
    /// Just entered the running condition: fetch once immediately, then poll on
    /// the interval (resume-once).
    FetchOnceThenPoll,
    /// Already running: continue polling on the interval (no extra fetch).
    Poll,
}

impl FleetPoll {
    /// A fresh controller — nothing running until [`Self::update`] is fed the
    /// first inputs.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the machine is currently in the running condition (active AND
    /// visible).
    pub fn is_running(&self) -> bool {
        self.active && self.visible
    }

    /// Feed the current `{active, visible}` inputs and get the action the loop
    /// should take. Transitions:
    /// - `running && !was_running` → [`PollAction::FetchOnceThenPoll`] (a fresh
    ///   entry: destination opened or tab refocused — one immediate fetch);
    /// - `running && was_running` → [`PollAction::Poll`] (steady interval);
    /// - `!running` → [`PollAction::Stop`] (and the resume latch resets, so the
    ///   next entry fetches once again).
    pub fn update(&mut self, active: bool, visible: bool) -> PollAction {
        self.active = active;
        self.visible = visible;
        let running = self.is_running();
        let action = match (running, self.was_running) {
            (true, false) => PollAction::FetchOnceThenPoll,
            (true, true) => PollAction::Poll,
            (false, _) => PollAction::Stop,
        };
        self.was_running = running;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_the_destination_fetches_once_then_polls() {
        let mut poll = FleetPoll::new();
        // Not active yet → stopped.
        assert_eq!(poll.update(false, true), PollAction::Stop);
        // Route becomes active and tab is visible → one immediate fetch.
        assert_eq!(poll.update(true, true), PollAction::FetchOnceThenPoll);
        // Steady state → plain polling, no extra fetch.
        assert_eq!(poll.update(true, true), PollAction::Poll);
    }

    #[test]
    fn backgrounding_stops_and_refocus_resumes_once() {
        let mut poll = FleetPoll::new();
        assert_eq!(poll.update(true, true), PollAction::FetchOnceThenPoll);
        assert_eq!(poll.update(true, true), PollAction::Poll);
        // Tab hidden → stop; no polling in the background.
        assert_eq!(poll.update(true, false), PollAction::Stop);
        assert_eq!(poll.update(true, false), PollAction::Stop);
        // Refocus → EXACTLY one resume-fetch, then steady polling.
        assert_eq!(poll.update(true, true), PollAction::FetchOnceThenPoll);
        assert_eq!(poll.update(true, true), PollAction::Poll);
    }

    #[test]
    fn leaving_the_destination_stops_and_reentry_resumes_once() {
        let mut poll = FleetPoll::new();
        assert_eq!(poll.update(true, true), PollAction::FetchOnceThenPoll);
        // Navigate away → stop.
        assert_eq!(poll.update(false, true), PollAction::Stop);
        // Return → resume-once again (the latch reset on stop).
        assert_eq!(poll.update(true, true), PollAction::FetchOnceThenPoll);
    }

    #[test]
    fn never_running_while_either_input_is_false() {
        let mut poll = FleetPoll::new();
        assert_eq!(poll.update(false, false), PollAction::Stop);
        assert_eq!(poll.update(true, false), PollAction::Stop);
        assert_eq!(poll.update(false, true), PollAction::Stop);
        assert!(!poll.is_running());
    }

    #[test]
    fn exactly_one_resume_fetch_per_reentry() {
        // The four transitions (activate, background, refocus, deactivate),
        // asserting the count of resume-fetches is exactly one per re-entry.
        let mut poll = FleetPoll::new();
        let mut fetches = 0;
        let tick = |poll: &mut FleetPoll, a, v, fetches: &mut u32| {
            if poll.update(a, v) == PollAction::FetchOnceThenPoll {
                *fetches += 1;
            }
        };
        tick(&mut poll, true, true, &mut fetches); // activate → fetch #1
        tick(&mut poll, true, true, &mut fetches); // steady
        tick(&mut poll, true, false, &mut fetches); // background
        tick(&mut poll, true, true, &mut fetches); // refocus → fetch #2
        tick(&mut poll, false, true, &mut fetches); // deactivate
        tick(&mut poll, true, true, &mut fetches); // re-enter → fetch #3
        assert_eq!(fetches, 3, "one resume-fetch per re-entry, never a backlog");
    }
}
