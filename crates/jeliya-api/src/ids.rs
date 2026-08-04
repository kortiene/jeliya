//! Opaque identifier domains. Each is a distinct newtype so a `RoomId` can
//! never be passed where a `SubjectId` belongs — protocol v2 defines these
//! as opaque strings, each a distinct domain, with no representation
//! guarantee (never assumed hexadecimal).

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Largest request id exactly representable by ordinary browser and Node JSON
/// numbers (`2^53 - 1`).
pub const MAX_REQUEST_ID: u64 = 9_007_199_254_740_991;

/// The request envelope's browser-safe integer identifier.
///
/// It is unique only while outstanding on one connection; that lifecycle
/// property is deliberately not modeled by this structural value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    /// Constructs a request id when `value` is in the canonical
    /// `0..=9_007_199_254_740_991` range.
    pub fn new(value: u64) -> Result<Self, RequestIdOutOfRange> {
        if value <= MAX_REQUEST_ID {
            Ok(Self(value))
        } else {
            Err(RequestIdOutOfRange { value })
        }
    }

    /// Returns the underlying browser-safe integer.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<u64> for RequestId {
    type Error = RequestIdOutOfRange;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RequestId> for u64 {
    fn from(value: RequestId) -> Self {
        value.get()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A request id exceeded the canonical browser-safe integer range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("request id {value} exceeds {MAX_REQUEST_ID}")]
pub struct RequestIdOutOfRange {
    /// The rejected integer.
    pub value: u64,
}

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a raw identifier. The value is opaque: no format
            /// validation is applied, because protocol v2 guarantees none.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            /// The raw opaque string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// Consumes into the raw string.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_id!(RoomId, "Opaque room identifier (`<room_id>`).");
opaque_id!(SubjectId, "Opaque subject identifier (`<subject_id>`).");
opaque_id!(DeviceId, "Opaque device identifier (`<device_id>`).");
opaque_id!(EventId, "Opaque event identifier (`<event_id>`).");
opaque_id!(InviteId, "Opaque invite identifier (`<invite_id>`).");
opaque_id!(FileId, "Opaque file identifier (`<file_id>`).");
opaque_id!(PipeId, "Opaque pipe identifier (`<pipe_id>`).");
opaque_id!(
    OpId,
    "Client-generated request deduplication key (`<op_id>`); envelope-level, never inside `in`."
);
