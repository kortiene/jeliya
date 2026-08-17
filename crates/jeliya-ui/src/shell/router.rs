//! The router (#178 §5.C, D1): the route *is* the navigation state, and there is
//! exactly one state machine.
//!
//! The shared, already-shipped [`jeliya_platform::navigation::Route`] model is
//! reused verbatim (#178 adds no second route type). The pure **decisions** —
//! mount canonicalization, fail-safe parsing, push-vs-replace, and once-per-
//! launch last-room restore — live here as host-testable functions. The Dioxus
//! [`use_route`] hook holds a `Signal<Route>` that is a *mirror* of the URL,
//! synchronized on every navigation, never an independent source of truth.
//!
//! Fail-safe (D7): the typed [`Route::parse`] is strict and returns `Err` for a
//! malformed or unknown path; the router maps `Err` → [`Route::Rooms`] and
//! *replaces* the URL to `/rooms` — "an unknown URL is a recoverable state, not
//! an error page", expressed through the stricter parser rather than a total one.

use jeliya_api::RoomId;
use jeliya_platform::navigation::{RoomDest, Route};

/// One navigation intent: where to go, and whether it **replaces** the current
/// history entry (§5.C step 4 / D4) rather than pushing a new one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NavIntent {
    /// The destination route.
    pub route: Route,
    /// `true` to replace (canonicalizing redirects, fail-safe rewrites); `false`
    /// to push (an ordinary user navigation).
    pub replace: bool,
}

impl NavIntent {
    /// A pushing navigation.
    pub fn push(route: Route) -> Self {
        Self {
            route,
            replace: false,
        }
    }

    /// A replacing navigation.
    pub fn replace(route: Route) -> Self {
        Self {
            route,
            replace: true,
        }
    }
}

/// The mount-time resolution of the initial URL (§5.C steps 1–2, 5).
///
/// Applied by the caller in order: seed the mirror signal to [`route`](Self::route),
/// then if [`replace_to`](Self::replace_to) is set write the canonical URL with a
/// **replace** (so Back never lands on a non-canonical URL the user never typed),
/// then if [`restore_push`](Self::restore_push) is set **push** the restored room
/// on top (so Back leaves the room for the list). The final visible route is
/// [`MountResolution::final_route`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MountResolution {
    /// The route the raw URL means after parsing/fallback (before restore).
    pub route: Route,
    /// The canonical route to rewrite the URL to via **replace**, when the raw
    /// spelling was non-canonical (`/`, `/rooms/`, `/rooms/:id`) or unparseable.
    pub replace_to: Option<Route>,
    /// The last-open room to **push** on top of the canonicalized `/rooms`, when
    /// the raw path was the bare root and a `LastRoom` preference is present.
    pub restore_push: Option<Route>,
}

impl MountResolution {
    /// The route actually shown after every mount action is applied.
    pub fn final_route(&self) -> Route {
        self.restore_push
            .clone()
            .or_else(|| self.replace_to.clone())
            .unwrap_or_else(|| self.route.clone())
    }
}

/// Parse `path` into a [`Route`], falling back to [`Route::Rooms`] on any
/// malformed or unknown path, and report whether the fallback fired (so the
/// caller can *replace* the URL to `/rooms`).
///
/// This is the fail-safe rule (D7): a strict parse `Err` becomes the recoverable
/// Rooms state, never an error page.
pub fn parse_or_rooms(path: &str) -> (Route, bool) {
    match Route::parse(path) {
        Ok(route) => (route, false),
        Err(_) => (Route::Rooms, true),
    }
}

/// Resolve the initial URL at mount (§5.C).
///
/// `raw_path` is the pre-parse `location.pathname` (no query/fragment — those
/// are not navigation state and are preserved by the binding's `url_for`).
/// `last_room` is the `LastRoom` preference, if any.
///
/// - A malformed/unknown path → Rooms, replace to `/rooms`, no restore.
/// - A non-canonical spelling of a valid route (`/`, `/rooms/`, `/rooms/:id`) →
///   the parsed route, replace to its canonical `to_path`.
/// - A canonical path → the parsed route, no replace.
/// - **Last-room restore** fires *only* when `raw_path` is the bare root `/`
///   (never `/rooms` or deeper): the URL is first replaced to `/rooms`, then the
///   restored room is pushed on top. An explicit route always wins.
pub fn resolve_mount(raw_path: &str, last_room: Option<&RoomId>) -> MountResolution {
    let is_bare_root = raw_path == "/" || raw_path.is_empty();
    let (route, fell_back) = parse_or_rooms(raw_path);

    // Canonicalize when the raw spelling differs from the parsed route's
    // canonical spelling, or when we fell back — always via replace.
    let canonical = route.to_path();
    let replace_to = if fell_back || raw_path != canonical {
        Some(route.clone())
    } else {
        None
    };

    // Restore only from the bare root, and only to a room (never off `/rooms` or
    // any deeper/explicit path). Pushed on top of the canonicalized `/rooms`.
    let restore_push = if is_bare_root {
        last_room.map(|id| Route::Room {
            room_id: id.clone(),
            dest: RoomDest::Activity,
        })
    } else {
        None
    };

    MountResolution {
        route,
        replace_to,
        restore_push,
    }
}

/// Whether navigating to `to` from the currently-shown `current` should push a
/// new history entry. Re-navigating to the path already shown pushes **no**
/// duplicate entry (Back must not become a double-press no-op — §5.A). A replace
/// still runs regardless; this governs only pushes.
pub fn should_push(current: &Route, to: &Route) -> bool {
    current.to_path() != to.to_path()
}

// The Dioxus hook. The whole `shell` module is `ui`-gated at the crate root
// (`lib.rs`), which is where target/feature selection lives (Decision-3), so
// this needs no feature gate of its own — the pure functions above and this hook
// compile together only under `ui`.
mod hook {
    use super::*;
    use dioxus::prelude::*;
    use futures::StreamExt;
    use jeliya_platform::lifecycle::{LifecycleDelivery, LifecycleEvent};

    use crate::PlatformServices;

    /// The router handle a component holds: the mirror signal and a navigate
    /// callback. Both are `Copy` (a `Signal` and a `Callback`), so they ride
    /// freely through the tree.
    #[derive(Clone, Copy)]
    pub struct RouteApi {
        /// The current route — a *mirror* of the URL, synchronized on every
        /// navigation. Read it to render; never write it directly (use
        /// [`RouteApi::navigate`], which keeps the URL and the mirror in step).
        pub route: Signal<Route>,
        /// Navigate, keeping the URL and the mirror in step. `pushState`/
        /// `replaceState` do not fire `popstate`, so the mirror is updated here
        /// by the caller (the retiring `useRoute` invariant).
        pub navigate: Callback<NavIntent>,
    }

    impl RouteApi {
        /// Push a route (an ordinary user navigation), deduplicating a
        /// navigate-to-current into a no-op push (a replace-equivalent) so Back
        /// is not stranded on a duplicate entry.
        pub fn push(&self, route: Route) {
            let replace = !should_push(&self.route.peek(), &route);
            self.navigate.call(NavIntent { route, replace });
        }

        /// Replace the current entry with `route` (canonicalizing redirects).
        pub fn replace(&self, route: Route) {
            self.navigate.call(NavIntent::replace(route));
        }
    }

    /// The router hook (§5.C): seed the mirror from the platform, canonicalize
    /// once at mount via *replace*, restore the last room from the bare root,
    /// then fold every platform `NavigationRequested` (browser Back/Forward/
    /// deep-link) back into the mirror.
    ///
    /// `raw_path` is the pre-parse `location.pathname` (supplied by composition,
    /// where the `web-sys` read is confined); `last_room` is the `LastRoom`
    /// preference. Both are read once at mount.
    pub fn use_route(
        services: PlatformServices,
        raw_path: String,
        last_room: Option<RoomId>,
    ) -> RouteApi {
        let resolution = use_hook(|| resolve_mount(&raw_path, last_room.as_ref()));
        let mut route = use_signal(|| resolution.route.clone());

        let navigate = {
            let services = services.clone();
            use_callback(move |intent: NavIntent| {
                if intent.replace {
                    services.navigation().navigate_replace(intent.route.clone());
                } else {
                    services.navigation().navigate(intent.route.clone());
                }
                route.set(intent.route);
            })
        };

        // Apply the mount canonicalization + restore exactly once.
        use_hook({
            let resolution = resolution.clone();
            move || {
                if let Some(canonical) = resolution.replace_to.clone() {
                    navigate.call(NavIntent::replace(canonical));
                }
                if let Some(restored) = resolution.restore_push.clone() {
                    navigate.call(NavIntent::push(restored));
                }
            }
        });

        // Fold browser Back/Forward/deep-link into the mirror. A
        // NavigationRequested carries the platform's already-parsed route (the
        // binding maps a popstate path through the same fail-safe parse), so the
        // mirror simply mirrors it — no second parse, no second state machine.
        {
            let services = services.clone();
            use_future(move || {
                let services = services.clone();
                async move {
                    let mut sub = services.lifecycle().subscribe();
                    while let Some(delivery) = sub.next().await {
                        if let LifecycleDelivery::Event(LifecycleEvent::NavigationRequested {
                            route: requested,
                        }) = delivery
                        {
                            route.set(requested);
                        }
                    }
                }
            });
        }

        RouteApi { route, navigate }
    }
}

pub use hook::{use_route, RouteApi};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_path_falls_back_to_rooms_and_replaces() {
        let res = resolve_mount("/nope/garbage", None);
        assert_eq!(res.route, Route::Rooms);
        assert_eq!(res.replace_to, Some(Route::Rooms));
        assert_eq!(res.restore_push, None);
        assert_eq!(res.final_route(), Route::Rooms);
    }

    #[test]
    fn bare_root_canonicalizes_to_rooms_via_replace() {
        let res = resolve_mount("/", None);
        assert_eq!(res.route, Route::Rooms);
        // `/` is a non-canonical spelling of Rooms → replace to `/rooms`.
        assert_eq!(res.replace_to, Some(Route::Rooms));
        assert_eq!(res.restore_push, None);
    }

    #[test]
    fn a_bare_room_canonicalizes_to_activity_via_replace() {
        let res = resolve_mount("/rooms/r-1", None);
        let activity = Route::Room {
            room_id: RoomId::new("r-1"),
            dest: RoomDest::Activity,
        };
        assert_eq!(res.route, activity);
        assert_eq!(res.replace_to, Some(activity));
    }

    #[test]
    fn a_canonical_path_needs_no_replace() {
        let res = resolve_mount("/rooms/r-1/people", None);
        assert_eq!(
            res.route,
            Route::Room {
                room_id: RoomId::new("r-1"),
                dest: RoomDest::People,
            }
        );
        assert_eq!(res.replace_to, None);
        assert_eq!(res.restore_push, None);
    }

    #[test]
    fn last_room_restores_only_from_the_bare_root() {
        let last = RoomId::new("r-9");
        // From `/`: canonicalize to `/rooms`, then push the restored room.
        let res = resolve_mount("/", Some(&last));
        assert_eq!(res.replace_to, Some(Route::Rooms));
        assert_eq!(
            res.restore_push,
            Some(Route::Room {
                room_id: last.clone(),
                dest: RoomDest::Activity,
            })
        );
        assert_eq!(
            res.final_route(),
            Route::Room {
                room_id: last.clone(),
                dest: RoomDest::Activity,
            }
        );

        // From explicit `/rooms`: NO restore — the explicit route wins.
        let res = resolve_mount("/rooms", Some(&last));
        assert_eq!(res.restore_push, None);
        assert_eq!(res.final_route(), Route::Rooms);

        // From a deeper explicit path: NO restore.
        let res = resolve_mount("/settings", Some(&last));
        assert_eq!(res.restore_push, None);
        assert_eq!(res.final_route(), Route::Settings);
    }

    #[test]
    fn should_push_dedupes_navigating_to_the_current_path() {
        let people = Route::Room {
            room_id: RoomId::new("r-1"),
            dest: RoomDest::People,
        };
        assert!(!should_push(&people, &people));
        assert!(should_push(&people, &Route::Rooms));
    }

    #[test]
    fn a_trailing_slash_is_canonicalized_via_replace() {
        let res = resolve_mount("/rooms/", None);
        assert_eq!(res.route, Route::Rooms);
        assert_eq!(res.replace_to, Some(Route::Rooms));
    }

    #[test]
    fn empty_path_is_treated_as_bare_root_and_restores() {
        // An empty pathname (the browser can return "" on some paths) is bare-root
        // eligible — same as "/" — so last-room restore fires.
        let last = RoomId::new("r-7");
        let res = resolve_mount("", Some(&last));
        assert_eq!(
            res.replace_to,
            Some(Route::Rooms),
            "empty path canonicalizes to /rooms via replace"
        );
        assert_eq!(
            res.restore_push,
            Some(Route::Room {
                room_id: last.clone(),
                dest: RoomDest::Activity,
            }),
            "empty path is restore-eligible like bare root"
        );
        assert_eq!(
            res.final_route(),
            Route::Room {
                room_id: last,
                dest: RoomDest::Activity
            },
        );
    }

    #[test]
    fn should_push_is_true_between_global_destinations() {
        assert!(should_push(&Route::Rooms, &Route::Fleet));
        assert!(should_push(&Route::Fleet, &Route::Settings));
        assert!(should_push(&Route::Settings, &Route::Rooms));
        // Same-destination is not a push.
        assert!(!should_push(&Route::Fleet, &Route::Fleet));
        assert!(!should_push(&Route::Settings, &Route::Settings));
    }

    #[test]
    fn nav_intent_push_and_replace_flags_are_correct() {
        let route = Route::Fleet;
        let push = NavIntent::push(route.clone());
        assert_eq!(push.route, route);
        assert!(!push.replace, "NavIntent::push must have replace=false");

        let rep = NavIntent::replace(route.clone());
        assert_eq!(rep.route, route);
        assert!(rep.replace, "NavIntent::replace must have replace=true");
    }

    #[test]
    fn last_room_is_not_restored_off_explicit_fleet_or_settings() {
        // An explicit global destination other than /rooms must not restore.
        let last = RoomId::new("r-5");
        for path in ["/fleet", "/settings"] {
            let res = resolve_mount(path, Some(&last));
            assert!(
                res.restore_push.is_none(),
                "restore must not fire from explicit path {path}"
            );
        }
    }
}
