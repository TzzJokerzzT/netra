//! Strongly typed identifiers.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a fresh random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            /// Parses an identifier from its canonical hyphenated form.
            ///
            /// # Errors
            ///
            /// Returns [`DomainError::InvalidId`] when the input is not a UUID.
            pub fn parse(value: &str) -> Result<Self, DomainError> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|_| DomainError::InvalidId(value.to_owned()))
            }

            /// Returns a short prefix for compact UI labels.
            #[must_use]
            pub fn short(self) -> String {
                self.0.to_string().chars().take(8).collect()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }
    };
}

define_id!(SessionId, "Identifier of a capture session.");
define_id!(
    ExchangeId,
    "Identifier of a single request/response exchange."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique() {
        assert_ne!(SessionId::new(), SessionId::new());
    }

    #[test]
    fn round_trips_through_text() {
        let id = ExchangeId::new();
        assert_eq!(ExchangeId::parse(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn rejects_non_uuid_text() {
        assert!(SessionId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn short_label_is_eight_chars() {
        assert_eq!(SessionId::new().short().len(), 8);
    }
}
