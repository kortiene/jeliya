//! The navigation capability (#174 §D10): the route is the navigation state.
//!
//! [`Navigation`] exposes the current [`Route`], a way to navigate, and a way
//! to hand a back gesture back to the platform. [`Route`] is a **typed, parsed**
//! route, not a raw string, so the router and the platform agree on one state
//! machine — the product contract's "the URL *is* the navigation state; no
//! second state machine may disagree with it". On the browser the
//! implementation is the history/hash route; on desktop/Android an in-memory
//! router.

use jeliya_api::RoomId;

/// A typed, parsed application route.
///
/// Parsing and rendering round-trip through [`Route::parse`] and
/// [`Route::to_path`], so a deep link and an in-app navigation share one
/// representation. Unknown paths fail to parse rather than silently becoming a
/// default — a second state machine that disagreed with the URL is exactly what
/// the product contract forbids.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Route {
    /// The bare root (the room list). The last-open-room restore applies here
    /// only, once per launch, and always loses to an explicit route.
    Root,
    /// One room's workbench.
    Room {
        /// The open room.
        room_id: RoomId,
    },
    /// The settings surface.
    Settings,
}

/// Why a path could not be parsed into a [`Route`].
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("unrecognized route path")]
pub struct RouteParseError;

impl Route {
    /// Parse a path (for example `/`, `/rooms/<id>`, `/settings`) into a
    /// [`Route`]. Fails closed on an unrecognized path rather than defaulting.
    pub fn parse(path: &str) -> Result<Self, RouteParseError> {
        let trimmed = path.trim_end_matches('/');
        match trimmed {
            "" => Ok(Route::Root),
            "/settings" => Ok(Route::Settings),
            other => {
                if let Some(id) = other.strip_prefix("/rooms/") {
                    if id.is_empty() || id.contains('/') {
                        return Err(RouteParseError);
                    }
                    Ok(Route::Room {
                        room_id: RoomId::new(id),
                    })
                } else {
                    Err(RouteParseError)
                }
            }
        }
    }

    /// Render the route as a canonical path, the inverse of [`Route::parse`].
    pub fn to_path(&self) -> String {
        match self {
            Route::Root => "/".to_owned(),
            Route::Room { room_id } => format!("/rooms/{room_id}"),
            Route::Settings => "/settings".to_owned(),
        }
    }

    /// Whether this is the bare root — the only route the last-open-room restore
    /// may replace.
    pub fn is_root(&self) -> bool {
        matches!(self, Route::Root)
    }
}

/// The navigation capability: read and change the one navigation state.
pub trait Navigation {
    /// The current route.
    fn route(&self) -> Route;

    /// Navigate to `route`. The route *is* the state; there is no second
    /// machine to keep in sync.
    fn navigate(&self, route: Route);

    /// Hand an unconsumed back gesture back to the platform (the explicit
    /// alternative to an in-app pop). A
    /// [`crate::lifecycle::LifecycleEvent::BackRequested`] the router does not
    /// consume must be resolved through this method (or
    /// [`crate::window::WindowActions::request_exit`]); it must **never**
    /// silently become an exit.
    fn hand_back_to_platform(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip_through_parse_and_to_path() {
        for route in [
            Route::Root,
            Route::Settings,
            Route::Room {
                room_id: RoomId::new("r-42"),
            },
        ] {
            let parsed = Route::parse(&route.to_path()).expect("round-trips");
            assert_eq!(parsed, route);
        }
    }

    #[test]
    fn unknown_paths_fail_closed() {
        assert!(Route::parse("/nope").is_err());
        assert!(Route::parse("/rooms/").is_err());
        assert!(Route::parse("/rooms/a/b").is_err());
    }

    #[test]
    fn only_root_is_root() {
        assert!(Route::Root.is_root());
        assert!(!Route::Settings.is_root());
    }
}
