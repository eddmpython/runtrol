//! An opaque body the courier carries without reading.

use core::fmt;

/// A UTF-8 body admitted under a byte ceiling. The courier validates its size and nothing else.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundedUtf8 {
    text: String,
}

/// A body larger than the ceiling. The text is not kept: refusing it is how the ceiling protects memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a body of {len} bytes exceeds the ceiling of {ceiling} bytes")]
pub struct BodyTooLarge {
    /// The size that was offered.
    pub len: usize,
    /// The ceiling it exceeded.
    pub ceiling: usize,
}

impl BoundedUtf8 {
    /// Admit `text` under `ceiling` bytes.
    ///
    /// # Errors
    ///
    /// A body longer than `ceiling` bytes is refused and dropped.
    pub fn new(text: String, ceiling: usize) -> Result<Self, BodyTooLarge> {
        let len = text.len();
        if len > ceiling {
            return Err(BodyTooLarge { len, ceiling });
        }
        Ok(Self { text })
    }

    /// The empty body: what a cancellation carries.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// The body's size in bytes: what the courier charges for it.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.text.len()
    }

    /// Whether there is nothing to carry.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The text, for the one place that hands it to its reader.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The text, owned, for the one place that hands it to its reader.
    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }
}

/// Bodies stay out of diagnostics: a body's `Debug` form is its size and nothing more.
impl fmt::Debug for BoundedUtf8 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedUtf8")
            .field("bytes", &self.text.len())
            .finish()
    }
}

impl serde::Serialize for BoundedUtf8 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

impl<'de> serde::Deserialize<'de> for BoundedUtf8 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text, crate::Limits::INITIAL.body_bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_past_the_ceiling_is_refused_and_one_at_the_ceiling_is_kept() {
        let at_ceiling = "x".repeat(16);
        let kept =
            BoundedUtf8::new(at_ceiling.clone(), 16).expect("sixteen bytes fit under sixteen");
        assert_eq!(kept.len(), 16);
        assert_eq!(kept.as_str(), at_ceiling);

        let too_large = BoundedUtf8::new("y".repeat(17), 16);
        assert_eq!(
            too_large,
            Err(BodyTooLarge {
                len: 17,
                ceiling: 16
            })
        );
    }

    #[test]
    fn bytes_are_counted_as_utf8_bytes_not_characters() {
        let korean = "안녕".to_owned();
        assert_eq!(korean.chars().count(), 2);
        assert!(BoundedUtf8::new(korean.clone(), 5).is_err());
        assert_eq!(BoundedUtf8::new(korean, 6).expect("six bytes fit").len(), 6);
    }

    #[test]
    fn a_body_never_appears_in_its_own_debug_form() {
        let body = BoundedUtf8::new("secret text".to_owned(), 64).expect("fits");
        let rendered = format!("{body:?}");
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(rendered.contains("11"), "{rendered}");
        assert!(BoundedUtf8::empty().is_empty());
        assert_eq!(body.into_string(), "secret text");
    }
}
