//! Source-neutral interface-pack validation failures and TOML locators.

use std::{fmt, ops::Range};

use toml_edit::{Document, Item, Table};

use super::{InterfaceAnchor, InterfaceSlot};

pub(super) type ValidationResult<T> = std::result::Result<T, ValidationError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ValidationError {
    message: String,
    location: Location,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Location {
    Pack {
        key: &'static str,
    },
    Anchor {
        id: String,
        key: &'static str,
    },
    Guard {
        anchor: String,
        index: usize,
        key: &'static str,
    },
    Slot {
        anchor: String,
        offset: i32,
        width: u8,
        key: &'static str,
    },
}

impl ValidationError {
    pub(super) fn pack(key: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: Location::Pack { key },
        }
    }

    pub(super) fn anchor(
        anchor: &InterfaceAnchor,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Anchor {
                id: anchor.id.clone(),
                key,
            },
        }
    }

    pub(super) fn guard(
        anchor: &InterfaceAnchor,
        index: usize,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Guard {
                anchor: anchor.id.clone(),
                index,
                key,
            },
        }
    }

    pub(super) fn slot(
        anchor: &InterfaceAnchor,
        slot: &InterfaceSlot,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Slot {
                anchor: anchor.id.clone(),
                offset: slot.offset,
                width: slot.width,
                key,
            },
        }
    }

    pub(super) fn span(&self, document: &Document<String>) -> Option<Range<usize>> {
        match &self.location {
            Location::Pack { key } => item_or_container_span(document.as_item(), key),
            Location::Anchor { id, key } => {
                let table = anchor_table(document, id)?;
                table_item_or_span(table, key)
            }
            Location::Guard { anchor, index, key } => {
                let table = anchor_table(document, anchor)?;
                let guard = table.get("guards")?.as_array_of_tables()?.get(*index)?;
                table_item_or_span(guard, key)
            }
            Location::Slot {
                anchor,
                offset,
                width,
                key,
            } => {
                let table = anchor_table(document, anchor)?;
                let slots = table.get("slots")?.as_array_of_tables()?;
                let slot = slots.iter().find(|slot| {
                    slot.get("offset").and_then(Item::as_integer) == Some(i64::from(*offset))
                        && slot.get("width").and_then(Item::as_integer) == Some(i64::from(*width))
                })?;
                table_item_or_span(slot, key)
            }
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

fn anchor_table<'a>(document: &'a Document<String>, id: &str) -> Option<&'a Table> {
    document
        .get("anchors")?
        .as_array_of_tables()?
        .iter()
        .find(|table| table.get("id").and_then(Item::as_str) == Some(id))
}

fn item_or_container_span(item: &Item, key: &str) -> Option<Range<usize>> {
    item.get(key).and_then(Item::span).or_else(|| item.span())
}

fn table_item_or_span(table: &Table, key: &str) -> Option<Range<usize>> {
    table.get(key).and_then(Item::span).or_else(|| table.span())
}
