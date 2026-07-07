//! The daemon<->UI error taxonomy from `docs/PROTOCOL.md`: every fallible core
//! operation returns a [`CoreError`] carrying one of the protocol's stable
//! `error.code` strings plus a human message and an optional next-action hint
//! (the CLI's IR-0303 convention).

use std::fmt;

/// The protocol `error.code` set (PROTOCOL.md "Envelope"). Mirrors the SDK/CLI
/// taxonomy where one exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A request parameter is missing or malformed.
    InvalidParams,
    /// No local identity exists yet (`identity.create` was never run).
    IdentityMissing,
    /// `identity.create` was called but an identity already exists.
    IdentityExists,
    /// The caller is not an active member of the room (or lacks the role the
    /// operation needs).
    NotAMember,
    /// No room with this id is known locally.
    RoomUnknown,
    /// The operation needs a live room session but the room is not open.
    RoomNotOpen,
    /// A ticket failed to decode or is bound to a different identity.
    BadTicket,
    /// The invite behind a ticket has expired.
    TicketExpired,
    /// No reachable provider holds the requested blob (honest best-effort P2P).
    FileUnavailable,
    /// Every reachable provider refused the connection (authorization wall).
    FileUnauthorized,
    /// Fetched bytes fail the independent BLAKE3 check — a hard stop.
    HashMismatch,
    /// A pipe operation was refused (non-loopback target, not owner/admin, …).
    PipeDenied,
    /// A required peer (admin, pipe owner) is unreachable.
    PeerUnreachable,
    /// Unexpected internal failure (bug signal).
    Internal,
}

impl ErrorKind {
    /// The stable wire code string.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidParams => "invalid_params",
            Self::IdentityMissing => "identity_missing",
            Self::IdentityExists => "identity_exists",
            Self::NotAMember => "not_a_member",
            Self::RoomUnknown => "room_unknown",
            Self::RoomNotOpen => "room_not_open",
            Self::BadTicket => "bad_ticket",
            Self::TicketExpired => "ticket_expired",
            Self::FileUnavailable => "file_unavailable",
            Self::FileUnauthorized => "file_unauthorized",
            Self::HashMismatch => "hash_mismatch",
            Self::PipeDenied => "pipe_denied",
            Self::PeerUnreachable => "peer_unreachable",
            Self::Internal => "internal",
        }
    }

    /// A generic next-action hint for kinds where one exists; call-site hints
    /// (via [`CoreError::with_hint`]) override this.
    #[must_use]
    pub fn default_hint(self) -> Option<&'static str> {
        match self {
            Self::IdentityMissing => Some("call identity.create first"),
            Self::IdentityExists => Some("the existing identity is already usable"),
            Self::RoomUnknown => Some("call room.create, or room.join with an invite ticket"),
            Self::RoomNotOpen => Some("call room.open first"),
            Self::BadTicket => Some("check the whole ticket was copied, or ask for a fresh invite"),
            Self::TicketExpired => Some("ask the admin for a fresh invite"),
            Self::FileUnavailable => {
                Some("ask a peer holding the file to open the room, then retry")
            }
            Self::FileUnauthorized => {
                Some("ask the admin to confirm your membership has synced, then retry")
            }
            Self::HashMismatch => Some("do not trust this file; ask for a fresh file.share"),
            Self::PeerUnreachable => Some("ask the peer to open the room, then retry"),
            Self::NotAMember => Some("ask the room admin for an invite"),
            Self::InvalidParams | Self::PipeDenied | Self::Internal => None,
        }
    }
}

/// A protocol-coded failure: `kind` maps to the wire `error.code`, `message`
/// is human-readable and secret-free, `hint` is a next-action line or `None`.
#[derive(Debug, Clone)]
pub struct CoreError {
    /// The protocol error code.
    pub kind: ErrorKind,
    /// Human-readable, secret-free description.
    pub message: String,
    /// Next-action line (IR-0303 convention) or `None`.
    pub hint: Option<String>,
}

impl CoreError {
    /// Build an error with the kind's default hint (if any).
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: kind.default_hint().map(str::to_owned),
        }
    }

    /// Replace the hint with a call-site-specific next action.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Shorthand for an [`ErrorKind::Internal`] failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// Shorthand for an [`ErrorKind::InvalidParams`] failure.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidParams, message)
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.code(), self.message)
    }
}

impl std::error::Error for CoreError {}

/// Convenience alias used across the core.
pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::{CoreError, ErrorKind};

    #[test]
    fn codes_match_the_protocol_table() {
        // Pinned against docs/PROTOCOL.md's error.code list.
        let expected = [
            (ErrorKind::InvalidParams, "invalid_params"),
            (ErrorKind::IdentityMissing, "identity_missing"),
            (ErrorKind::IdentityExists, "identity_exists"),
            (ErrorKind::NotAMember, "not_a_member"),
            (ErrorKind::RoomUnknown, "room_unknown"),
            (ErrorKind::RoomNotOpen, "room_not_open"),
            (ErrorKind::BadTicket, "bad_ticket"),
            (ErrorKind::TicketExpired, "ticket_expired"),
            (ErrorKind::FileUnavailable, "file_unavailable"),
            (ErrorKind::FileUnauthorized, "file_unauthorized"),
            (ErrorKind::HashMismatch, "hash_mismatch"),
            (ErrorKind::PipeDenied, "pipe_denied"),
            (ErrorKind::PeerUnreachable, "peer_unreachable"),
            (ErrorKind::Internal, "internal"),
        ];
        for (kind, code) in expected {
            assert_eq!(kind.code(), code);
        }
    }

    #[test]
    fn with_hint_overrides_default() {
        let err = CoreError::new(ErrorKind::RoomNotOpen, "room x is not open")
            .with_hint("open it with room.open");
        assert_eq!(err.hint.as_deref(), Some("open it with room.open"));
    }
}
