//! The identifiers the courier routes by.
//!
//! Every one is a UUIDv7 in its canonical lowercase hyphenated spelling, so a message, a call, a managed session
//! and a room each sort by the moment they were minted, and a transport carries any of them as 36 bytes of ASCII.
//! Parsing is strict: another spelling of the same bytes is refused, so one identity has exactly one text and a
//! duplicate cannot hide behind a capital letter or a pair of braces.

use core::fmt;
use core::str::FromStr;

use uuid::Uuid;

/// Why a piece of text is not a courier identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The text is not a UUID at all.
    #[error("not a UUID")]
    Malformed,
    /// A UUID of another version: it would not sort by the moment it was minted.
    #[error("UUID version {0} where version 7 is required")]
    Version(usize),
    /// A UUID spelled other than lowercase hyphenated.
    #[error("not the canonical lowercase hyphenated UUID spelling")]
    Spelling,
}

fn parse_canonical_v7(text: &str) -> Result<Uuid, IdError> {
    let id = Uuid::parse_str(text).map_err(|_malformed| IdError::Malformed)?;
    if id.get_version_num() != 7 {
        return Err(IdError::Version(id.get_version_num()));
    }
    if id.hyphenated().to_string() != text {
        return Err(IdError::Spelling);
    }
    Ok(id)
}

macro_rules! courier_identity {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint a fresh identifier, ordered after every one minted before it.
            #[must_use]
            pub fn now() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(text: &str) -> Result<Self, IdError> {
                parse_canonical_v7(text).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0.as_hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self.0.as_hyphenated())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

courier_identity! {
    /// One envelope. Sending the same identifier twice is a duplicate, not a second message.
    MessageId
}

courier_identity! {
    /// One request and the single reply that answers it.
    CallId
}

courier_identity! {
    /// One Runtrol-managed process for as long as the Runtime that started it runs. The Runtime hands the
    /// process its identifier at launch; it is never a provider-native conversation identifier.
    ManagedSessionId
}

courier_identity! {
    /// One bounded dialogue room. Rooms open in a later stamp; the identifier is defined with the others so the
    /// envelope layout does not change when they do.
    RoomId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_is_only_its_canonical_uuid_v7_text() {
        let minted = MessageId::now();
        let text = minted.to_string();
        assert_eq!(text.parse::<MessageId>(), Ok(minted));
        assert_eq!(text.len(), 36);

        assert_eq!(
            text.to_uppercase().parse::<MessageId>(),
            Err(IdError::Spelling),
            "the same bytes in capitals are another text, not the same identifier"
        );
        assert_eq!(
            format!("{{{text}}}").parse::<MessageId>(),
            Err(IdError::Spelling)
        );
        assert_eq!(
            text.replace('-', "").parse::<MessageId>(),
            Err(IdError::Spelling)
        );
        assert_eq!(
            Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000)
                .hyphenated()
                .to_string()
                .parse::<CallId>(),
            Err(IdError::Version(4))
        );
        assert_eq!(
            Uuid::nil().hyphenated().to_string().parse::<RoomId>(),
            Err(IdError::Version(0))
        );
        assert_eq!("".parse::<ManagedSessionId>(), Err(IdError::Malformed));
        assert_eq!(
            "not-an-identifier".parse::<ManagedSessionId>(),
            Err(IdError::Malformed)
        );
    }

    #[test]
    fn identifiers_sort_by_the_moment_they_were_minted() {
        let first = CallId::now();
        let second = CallId::now();
        assert!(first <= second);
        assert_eq!(format!("{first:?}"), format!("CallId({first})"));
    }
}
