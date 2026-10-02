//! Shared, borrowed IEEE 802.11 information-element framing.
//!
//! This module validates only the ID/Length/body envelope. Element identities,
//! body layouts and cardinality rules belong to the protocol using the list.

/// ID and Length octets preceding an information element or subelement.
pub const ELEMENT_HEADER_LEN: usize = 2;
pub const ELEMENT_LENGTH_OFFSET: usize = 1;
pub const MAX_ELEMENT_BODY_LEN: usize = u8::MAX as usize;
pub const MAX_ENCODED_ELEMENT_LEN: usize = ELEMENT_HEADER_LEN + MAX_ELEMENT_BODY_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementError {
    Truncated,
    Duplicate(u8),
}

/// A sequence whose complete framing has been validated, including its tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elements<'a>(&'a [u8]);

impl<'a> Elements<'a> {
    pub const EMPTY: Self = Self(&[]);

    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElementError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let header = rest
                .get(..ELEMENT_HEADER_LEN)
                .ok_or(ElementError::Truncated)?;
            let length = ELEMENT_HEADER_LEN + usize::from(header[ELEMENT_LENGTH_OFFSET]);
            rest = rest.get(length..).ok_or(ElementError::Truncated)?;
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub fn iter(self) -> impl Iterator<Item = Element<'a>> {
        let mut rest = self.0;
        core::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let length = ELEMENT_HEADER_LEN + usize::from(rest[ELEMENT_LENGTH_OFFSET]);
            let (encoded, remaining) = rest.split_at(length);
            rest = remaining;
            Some(Element {
                id: encoded[0],
                body: &encoded[ELEMENT_HEADER_LEN..],
                encoded,
            })
        })
    }

    /// Apply a protocol's singleton rule without discarding duplicate fields.
    pub fn unique(self, id: u8) -> Result<Option<Element<'a>>, ElementError> {
        let mut found = None;
        for element in self.iter().filter(|element| element.id == id) {
            if found.replace(element).is_some() {
                return Err(ElementError::Duplicate(id));
            }
        }
        Ok(found)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: u8,
    pub body: &'a [u8],
    encoded: &'a [u8],
}

impl<'a> Element<'a> {
    /// Includes the header, as required by integrity-protected IE exchanges.
    pub const fn encoded(self) -> &'a [u8] {
        self.encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_tail_and_singletons_are_checked_without_losing_unknown_fields() {
        assert_eq!(Elements::parse(&[1, 0, 2]), Err(ElementError::Truncated));
        assert_eq!(Elements::parse(&[1, 2, 3]), Err(ElementError::Truncated));
        let bytes = [200, 2, 0xff, 0, 1, 0, 200, 1, 7];
        let elements = Elements::parse(&bytes).unwrap();
        assert_eq!(elements.as_bytes(), bytes);
        assert_eq!(elements.iter().next().unwrap().encoded(), &bytes[..4]);
        assert_eq!(elements.unique(200), Err(ElementError::Duplicate(200)));
        assert_eq!(elements.unique(1).unwrap().unwrap().body, &[]);
        assert_eq!(elements.unique(2).unwrap(), None);
    }
}
