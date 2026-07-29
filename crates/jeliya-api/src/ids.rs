//! Opaque identifier domains. Each is a distinct newtype so a `RoomId` can
//! never be passed where a `SubjectId` belongs — protocol v2 defines these
//! as opaque strings, each a distinct domain, with no representation
//! guarantee (never assumed hexadecimal).

use serde::{Deserialize, Serialize};
use std::fmt;

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
