//! The navigation capability (#174 §D10): the route is the navigation state.
//!
//! [`Navigation`] exposes the current [`Route`], a way to navigate, and a way
//! to hand a back gesture back to the platform. [`Route`] is a **typed, parsed**
//! route, not a raw string, so the router and the platform agree on one state
//! machine — the product contract's "the URL *is* the navigation state; no
//! second state machine may disagree with it". On the browser the
//! implementation is the history/hash route; on desktop/Android an in-memory
//! router.
//!
//! The route family is the canonical one from
//! `docs/product-behavior-contract.md` §"Canonical routes" (retained verbatim,
//! identical on every platform): `/rooms`, `/rooms/:roomId/<dest>` for the five
//! room destinations, `/fleet`, `/settings` — plus the item-selecting fourth
//! segment under Files/Pipes (#67), the only destinations where a fourth
//! segment is navigation state. `:roomId` is the protocol `room_id` verbatim
//! and percent-encoded, so [`Route::to_path`] spells `/rooms/blake3%3A…`
//! byte-identically to the shipped web shell's `encodeURIComponent`.

use jeliya_api::{FileId, PipeId, RoomId, SubjectId};

/// One room's destination — the room tools, in the product contract's
/// tab-strip order. `Activity` is the room's workspace, a real destination,
/// not a synonym for "a room is selected".
///
/// [`RoomDest::Files`] and [`RoomDest::Pipes`] select an individual item by its
/// opaque id (#67); [`RoomDest::People`] and [`RoomDest::Agents`] select an
/// individual **member/agent** by its [`SubjectId`] (#180 Q1). In every case
/// the selected item is part of the *type*, so a member selection under Files —
/// or a file selection under People — is unrepresentable rather than merely
/// rejected, and a selection is a genuinely stable deep link that Back closes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RoomDest {
    /// The room's workspace.
    Activity,
    /// Members and presence, optionally with one member selected.
    People {
        /// The selected member, when the route deep-links one.
        item: Option<SubjectId>,
    },
    /// Agents and runs, optionally with one agent selected (its run history
    /// open).
    Agents {
        /// The selected agent, when the route deep-links one.
        item: Option<SubjectId>,
    },
    /// Files shared in this room, optionally with one file open.
    Files {
        /// The selected file, when the route deep-links one.
        item: Option<FileId>,
    },
    /// Pipes exposed in this room, optionally with one pipe open.
    Pipes {
        /// The selected pipe, when the route deep-links one.
        item: Option<PipeId>,
    },
}

/// A typed, parsed application route — the canonical route family.
///
/// Parsing and rendering round-trip through [`Route::parse`] and
/// [`Route::to_path`], so a deep link and an in-app navigation share one
/// representation. Unknown paths fail to parse rather than silently becoming a
/// default — a second state machine that disagreed with the URL is exactly what
/// the product contract forbids. This parse is deliberately **stricter** than
/// the shipped web shell's total parse (which normalizes unknown room
/// destinations to Activity): the router maps `Err` to the Rooms recovery
/// state, so the user-visible recovery is equivalent while nothing here
/// silently invents a destination.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Route {
    /// The room list (no room selected). `/` canonicalizes here — the bare
    /// root is not a destination of its own.
    Rooms,
    /// One room's workbench, always at a concrete destination —
    /// `/rooms/:roomId` canonicalizes to that room's Activity.
    Room {
        /// The open room (the protocol `room_id`, verbatim — never a name, an
        /// index, or a short id).
        room_id: RoomId,
        /// The room destination.
        dest: RoomDest,
    },
    /// The agent fleet surface.
    Fleet,
    /// The settings surface.
    Settings,
}

/// Why a path could not be parsed into a [`Route`].
///
/// Deliberately payload-free (§K1): the offending path may carry a room id or
/// a filename, and an error string must not leak either.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("unrecognized route path")]
pub struct RouteParseError;

/// Percent-decode one path segment, strictly: a malformed escape (`%zz`, a
/// truncated `%A`) or non-UTF-8 decoded bytes fail closed instead of passing
/// through — a bad URL must become a parse error the router can recover from,
/// never a silently mangled room id.
fn decode_segment(segment: &str) -> Result<String, RouteParseError> {
    let segment = &unescape_exceptional_segment(segment);
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16));
            let lo = bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                }
                _ => return Err(RouteParseError),
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| RouteParseError)
}

/// Whether `segment` is zero or more `.` followed by zero or more `!` — the
/// closed family [`escape_exceptional_segment`] shifts.
///
/// It contains exactly the two spellings a path segment cannot carry — the
/// empty string, and the URL *dot segments* `.` and `..` — plus every value the
/// escape maps them onto, which is what keeps the shift reversible.
fn is_exceptional_family(segment: &str) -> bool {
    let dots = segment.len() - segment.trim_start_matches('.').len();
    segment[dots..].bytes().all(|b| b == b'!')
}

/// Make the path segments a URL cannot carry representable, reversibly.
///
/// An id is protocol-supplied and the `jeliya_api` newtypes apply no
/// representation validation, so two spellings break [`Route::to_path`]'s
/// promise to be the inverse of [`Route::parse`]:
///
/// - a `RoomId` of `..` emits `/rooms/../activity`, which a browser normalizes
///   to `/activity` before the router ever sees it — the room silently lost;
/// - an **empty** id emits `/rooms//activity`, which `parse` rejects outright,
///   and an empty selected `FileId` emits `/rooms/x/files/`, which parses back
///   as *no selection*.
///
/// Percent-encoding cannot save the first: the URL Standard defines a
/// double-dot segment as `..`, `.%2e`, `%2e.`, or `%2e%2e` (ASCII
/// case-insensitive), and a single-dot segment likewise, so every spelling of
/// the dots normalizes. The bytes themselves have to change. A `!` is
/// appended — unreserved in `encodeURIComponent`, so it survives encoding
/// literally — and the whole `.`\*`!`\* family shifts by one, which is what
/// keeps the escape reversible: `""` spells `!`, `.` spells `.!`, `..` spells
/// `..!`, `!` spells `!!`.
///
/// This is the **one** deliberate divergence from `encodeURIComponent`, and it
/// touches only segments no real id has: every `blake3:…` id, and anything
/// containing a character other than `.` and `!`, is spelled byte-identically
/// to the web shell as before. The shipped shell cannot represent these ids
/// correctly either — this makes them round-trip here rather than silently
/// losing the route.
fn escape_exceptional_segment(segment: &str) -> std::borrow::Cow<'_, str> {
    if is_exceptional_family(segment) {
        std::borrow::Cow::Owned(format!("{segment}!"))
    } else {
        std::borrow::Cow::Borrowed(segment)
    }
}

/// The inverse of [`escape_exceptional_segment`], applied before
/// percent-decoding.
fn unescape_exceptional_segment(segment: &str) -> std::borrow::Cow<'_, str> {
    match segment.strip_suffix('!') {
        Some(rest) if is_exceptional_family(segment) => std::borrow::Cow::Owned(rest.to_owned()),
        _ => std::borrow::Cow::Borrowed(segment),
    }
}

/// Percent-encode one path segment with **exactly** `encodeURIComponent`'s
/// unreserved set (`A–Z a–z 0–9 - _ . ! ~ * ' ( )`) and uppercase hex, so the
/// spelling is byte-identical to the web shell's `routePath` — room ids are
/// `blake3:…`, and both stacks must spell `/rooms/blake3%3A…` the same way.
/// Hand-rolled (the crate's only library dependency is `jeliya-api`; a
/// percent-encoding crate is not worth a new edge in the audited graph).
fn encode_segment(segment: &str) -> String {
    let segment = &escape_exceptional_segment(segment);
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            other => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                // Uppercase hex, as encodeURIComponent emits.
                out.push('%');
                out.push(HEX[usize::from(other >> 4)] as char);
                out.push(HEX[usize::from(other & 0xF)] as char);
            }
        }
    }
    out
}

impl Route {
    /// Parse a path (for example `/rooms`, `/rooms/:roomId/files/:fileId`,
    /// `/settings`) into a [`Route`]. Fails closed on an unrecognized path,
    /// a malformed percent-escape, or an empty interior segment rather than
    /// defaulting: `/rooms//activity` must not invent a room named
    /// "activity" (the web shell's `filter(Boolean)` would; this parser
    /// refuses instead). One trailing slash is tolerated as spelling noise.
    ///
    /// Canonicalizing rules from the product contract are applied here:
    /// `/` parses to [`Route::Rooms`], and `/rooms/:roomId` parses to that
    /// room's Activity.
    pub fn parse(path: &str) -> Result<Self, RouteParseError> {
        let rest = path.strip_prefix('/').ok_or(RouteParseError)?;
        let mut raw: Vec<&str> = rest.split('/').collect();
        // Tolerate exactly one trailing empty segment (a trailing slash);
        // any empty segment left after that is an interior hole — fail closed.
        if raw.last() == Some(&"") {
            raw.pop();
        }
        if raw.iter().any(|segment| segment.is_empty()) {
            return Err(RouteParseError);
        }
        let mut decoded = Vec::with_capacity(raw.len());
        for segment in raw {
            decoded.push(decode_segment(segment)?);
        }
        let segments: Vec<&str> = decoded.iter().map(String::as_str).collect();
        match segments.as_slice() {
            // Contract rule 1: `/` resolves to `/rooms`.
            [] | ["rooms"] => Ok(Route::Rooms),
            ["fleet"] => Ok(Route::Fleet),
            ["settings"] => Ok(Route::Settings),
            // Contract rule 1: `/rooms/:roomId` resolves to Activity.
            ["rooms", id] => Ok(Route::Room {
                room_id: RoomId::new(*id),
                dest: RoomDest::Activity,
            }),
            ["rooms", id, dest] => {
                let dest = match *dest {
                    "activity" => RoomDest::Activity,
                    "people" => RoomDest::People { item: None },
                    "agents" => RoomDest::Agents { item: None },
                    "files" => RoomDest::Files { item: None },
                    "pipes" => RoomDest::Pipes { item: None },
                    _ => return Err(RouteParseError),
                };
                Ok(Route::Room {
                    room_id: RoomId::new(*id),
                    dest,
                })
            }
            // A 4th segment is the selected item, under the four
            // item-bearing destinations. People/Agents select a member/agent
            // by SubjectId (#180 Q1); Files/Pipes select a file/pipe (#67).
            // Activity has no item, so `/rooms/:id/activity/:x` fails closed.
            ["rooms", id, "people", item] => Ok(Route::Room {
                room_id: RoomId::new(*id),
                dest: RoomDest::People {
                    item: Some(SubjectId::new(*item)),
                },
            }),
            ["rooms", id, "agents", item] => Ok(Route::Room {
                room_id: RoomId::new(*id),
                dest: RoomDest::Agents {
                    item: Some(SubjectId::new(*item)),
                },
            }),
            ["rooms", id, "files", item] => Ok(Route::Room {
                room_id: RoomId::new(*id),
                dest: RoomDest::Files {
                    item: Some(FileId::new(*item)),
                },
            }),
            ["rooms", id, "pipes", item] => Ok(Route::Room {
                room_id: RoomId::new(*id),
                dest: RoomDest::Pipes {
                    item: Some(PipeId::new(*item)),
                },
            }),
            _ => Err(RouteParseError),
        }
    }

    /// Render the route as its **canonical** path, the inverse of
    /// [`Route::parse`]: one destination, one spelling. [`Route::Rooms`]
    /// renders as `/rooms` (never `/` — the bare root is not a destination),
    /// and every id segment is percent-encoded byte-identically to the web
    /// shell's `encodeURIComponent` spelling.
    pub fn to_path(&self) -> String {
        match self {
            Route::Rooms => "/rooms".to_owned(),
            Route::Fleet => "/fleet".to_owned(),
            Route::Settings => "/settings".to_owned(),
            Route::Room { room_id, dest } => {
                let mut path = format!("/rooms/{}", encode_segment(room_id.as_str()));
                match dest {
                    RoomDest::Activity => path.push_str("/activity"),
                    RoomDest::People { item } => {
                        path.push_str("/people");
                        if let Some(item) = item {
                            path.push('/');
                            path.push_str(&encode_segment(item.as_str()));
                        }
                    }
                    RoomDest::Agents { item } => {
                        path.push_str("/agents");
                        if let Some(item) = item {
                            path.push('/');
                            path.push_str(&encode_segment(item.as_str()));
                        }
                    }
                    RoomDest::Files { item } => {
                        path.push_str("/files");
                        if let Some(item) = item {
                            path.push('/');
                            path.push_str(&encode_segment(item.as_str()));
                        }
                    }
                    RoomDest::Pipes { item } => {
                        path.push_str("/pipes");
                        if let Some(item) = item {
                            path.push('/');
                            path.push_str(&encode_segment(item.as_str()));
                        }
                    }
                }
                path
            }
        }
    }

    /// Whether this is the rooms list. Note the last-open-room **restore**
    /// eligibility ("once per launch, only from the bare root") is *not* this
    /// predicate: `/` and `/rooms` both parse to [`Route::Rooms`], but an
    /// explicit `/rooms` must not restore — restore eligibility is a
    /// launch-time check on the raw pre-parse path in the platform bootstrap,
    /// where the `/`-vs-`/rooms` spelling still exists.
    pub fn is_rooms(&self) -> bool {
        matches!(self, Route::Rooms)
    }

    /// The room this route selects, if any (the web shell's `routeRoomId`).
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Route::Room { room_id, .. } => Some(room_id),
            _ => None,
        }
    }
}

/// The navigation capability: read and change the one navigation state.
pub trait Navigation {
    /// The current route.
    fn route(&self) -> Route;

    /// Navigate to `route`. The route *is* the state; there is no second
    /// machine to keep in sync. This **pushes** a history entry, so Back
    /// returns to where the user was.
    fn navigate(&self, route: Route);

    /// Navigate to `route`, **replacing** the current history entry rather than
    /// pushing a new one (#178 D4). Canonicalizing redirects (`/`→`/rooms`),
    /// a malformed URL rewritten to its recoverable state, and a legacy URL
    /// rewrite must all use replace, so Back never walks through a state the
    /// user never performed. Defaulted to [`Navigation::navigate`] so every
    /// existing implementation keeps compiling; only a platform whose history
    /// distinguishes push from replace (the browser's `history.replaceState`)
    /// overrides it.
    fn navigate_replace(&self, route: Route) {
        self.navigate(route);
    }

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
    fn every_canonical_route_parses_and_round_trips() {
        // The full table from docs/product-behavior-contract.md §"Canonical
        // routes", plus the #67 item routes. Each parses Ok and round-trips
        // byte-identically through to_path.
        for path in [
            "/rooms",
            "/rooms/r-1/activity",
            "/rooms/r-1/people",
            "/rooms/r-1/agents",
            "/rooms/r-1/people/blake3%3Amember",
            "/rooms/r-1/agents/blake3%3Aagent",
            "/rooms/r-1/files",
            "/rooms/r-1/pipes",
            "/rooms/r-1/files/f-9",
            "/rooms/r-1/pipes/p-3",
            "/fleet",
            "/settings",
        ] {
            let route = Route::parse(path).expect("canonical route parses");
            assert_eq!(route.to_path(), path, "round-trips byte-identically");
        }
    }

    /// An id is protocol-supplied and unvalidated, so `..` and `""` are both
    /// representable. Emitting `/rooms/../activity` would let a browser
    /// normalize the room away before the router saw it, and `/rooms//activity`
    /// does not parse at all — so `to_path` would not be the inverse of
    /// `parse`. No percent-encoding saves the dots (the URL Standard treats
    /// `%2e` exactly like `.`), so the whole `.`*`!`* family shifts by one.
    #[test]
    fn exceptional_ids_round_trip_as_nonempty_non_dot_segments() {
        for id in ["", ".", "..", "...", "!", "!!", ".!", "..!", "..!!"] {
            let route = Route::Room {
                room_id: RoomId::new(id),
                dest: RoomDest::Activity,
            };
            let path = route.to_path();
            let segment = path
                .strip_prefix("/rooms/")
                .and_then(|rest| rest.strip_suffix("/activity"))
                .expect("a room path");
            assert!(!segment.is_empty(), "{id:?} encoded to an empty segment");
            assert!(
                !segment.bytes().all(|b| b == b'.'),
                "{id:?} encoded to the dot segment {segment:?}"
            );
            assert_eq!(
                Route::parse(&path).expect("the escaped spelling parses"),
                route,
                "{id:?} must round-trip"
            );
        }
    }

    /// The same for a *selected item*, where an empty id previously round-tripped
    /// to `None` — a silently different route rather than a parse failure.
    #[test]
    fn an_exceptional_selected_item_id_round_trips() {
        for id in ["", ".", ".."] {
            let route = Route::Room {
                room_id: RoomId::new("r-1"),
                dest: RoomDest::Files {
                    item: Some(FileId::new(id)),
                },
            };
            let path = route.to_path();
            assert!(!path.ends_with('/'), "{id:?} left a trailing empty segment");
            assert_eq!(
                Route::parse(&path).expect("parses"),
                route,
                "{id:?} must round-trip as a selection, not as None"
            );
        }
    }

    /// The escape touches ONLY the `.`*`!`* family; every other id keeps the
    /// exact `encodeURIComponent` spelling. Asserted against literal expected
    /// paths — comparing `to_path` to `encode_segment` would just be comparing
    /// the encoder to itself.
    #[test]
    fn the_escape_leaves_ordinary_ids_untouched() {
        for (id, expected) in [
            ("blake3:abc", "/rooms/blake3%3Aabc/activity"),
            ("a..b", "/rooms/a..b/activity"),
            ("..hidden", "/rooms/..hidden/activity"),
            ("a!", "/rooms/a!/activity"),
            (".a.", "/rooms/.a./activity"),
            ("r-1", "/rooms/r-1/activity"),
        ] {
            let route = Route::Room {
                room_id: RoomId::new(id),
                dest: RoomDest::Activity,
            };
            assert_eq!(route.to_path(), expected, "{id:?}");
            assert_eq!(Route::parse(expected).expect("parses"), route, "{id:?}");
        }
    }

    #[test]
    fn room_ids_percent_encode_byte_identically_to_the_web_shell() {
        // Room ids are `blake3:…`; encodeURIComponent spells the colon %3A
        // (uppercase hex), and both stacks must agree byte-for-byte.
        let route = Route::Room {
            room_id: RoomId::new("blake3:abc"),
            dest: RoomDest::Activity,
        };
        assert_eq!(route.to_path(), "/rooms/blake3%3Aabc/activity");
        assert_eq!(
            Route::parse("/rooms/blake3%3Aabc/activity").expect("parses"),
            route,
            "the encoded spelling parses back to the same route"
        );
        // The full encodeURIComponent unreserved set passes through verbatim.
        let unreserved = RoomId::new("AZaz09-_.!~*'()");
        let route = Route::Room {
            room_id: unreserved,
            dest: RoomDest::People { item: None },
        };
        assert_eq!(route.to_path(), "/rooms/AZaz09-_.!~*'()/people");
    }

    #[test]
    fn canonicalizing_rules_apply_on_parse() {
        // Rule 1: `/` resolves to `/rooms`; `/rooms/:id` to Activity.
        assert_eq!(Route::parse("/").expect("root parses"), Route::Rooms);
        assert_eq!(
            Route::parse("/rooms/x").expect("bare room parses"),
            Route::Room {
                room_id: RoomId::new("x"),
                dest: RoomDest::Activity,
            }
        );
        // The canonical spelling of Rooms is `/rooms`, never `/`.
        assert_eq!(Route::Rooms.to_path(), "/rooms");
        // One trailing slash is tolerated as spelling noise.
        assert_eq!(Route::parse("/rooms/").expect("parses"), Route::Rooms);
    }

    #[test]
    fn malformed_and_unknown_paths_fail_closed() {
        // Malformed percent-escapes must not pass through silently.
        assert!(Route::parse("/rooms/%zz/activity").is_err());
        assert!(Route::parse("/rooms/%A/activity").is_err());
        // An empty interior segment must not invent room "activity".
        assert!(Route::parse("/rooms//activity").is_err());
        // An item segment under the item-free Activity destination is
        // unrepresentable (People/Agents DO carry an item — #180 Q1).
        assert!(Route::parse("/rooms/x/activity/extra").is_err());
        // Unknown forms fail closed — never a silently defaulted route.
        assert!(Route::parse("/nope").is_err());
        assert!(Route::parse("/rooms/x/unknown").is_err());
        assert!(Route::parse("/rooms/x/files/f/extra").is_err());
        assert!(Route::parse("/rooms/x/people/m/extra").is_err());
        assert!(Route::parse("rooms").is_err(), "a path starts with '/'");
    }

    #[test]
    fn is_rooms_and_room_id_read_the_state() {
        assert!(Route::Rooms.is_rooms());
        assert!(!Route::Settings.is_rooms());
        let room = Route::Room {
            room_id: RoomId::new("r-1"),
            dest: RoomDest::Agents { item: None },
        };
        assert!(!room.is_rooms());
        assert_eq!(room.room_id().map(RoomId::as_str), Some("r-1"));
        assert_eq!(Route::Fleet.room_id(), None);
    }
}
