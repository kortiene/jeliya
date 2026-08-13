//! A wrapper that renders its inner value as `<redacted>` in every `Debug` and
//! `Display`, so a stray `{:?}` on a struct holding a secret can never spill it
//! into a log line. Mirrors `jeliya_client::kernel::diag::Redacted` and the
//! #159 spike's private `auth_token` discipline: the token reaches the native
//! transport and nothing else.

use std::fmt;

use serde::{Deserialize, Deserializer};

/// A `Debug`/`Display` shield for a secret. The inner value is reachable only
/// through [`Redacted::expose`], which every read of the secret must call — so
/// reads are greppable and formatting is safe by default.
#[derive(Clone)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wrap a value so it is redacted in all formatting.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Read the underlying secret. The only door to the value; a reviewer greps
    /// for `.expose()` to enumerate every place the secret is read.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Redacted<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize the inner value, then wrap it — the portfile's `auth_token`
        // is redacted the instant it is read, before it can be `Debug`-printed.
        T::deserialize(deserializer).map(Redacted)
    }
}

#[cfg(test)]
mod tests {
    use super::Redacted;

    #[test]
    fn debug_and_display_never_reveal_the_secret() {
        let secret = Redacted::new(String::from("64hexcharsofdaemonsecret"));
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        // The value is still reachable for the one legitimate reader.
        assert_eq!(secret.expose(), "64hexcharsofdaemonsecret");
    }

    #[test]
    fn deserialize_wraps_and_redacts() {
        let wrapped: Redacted<String> =
            serde_json::from_str("\"tok-abc123\"").expect("string deserializes");
        assert_eq!(format!("{wrapped:?}"), "<redacted>");
        assert_eq!(wrapped.expose(), "tok-abc123");
    }
}
