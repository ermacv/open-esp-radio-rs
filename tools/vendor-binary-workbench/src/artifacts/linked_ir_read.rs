//! Strict typed consumer for the persistent linked-IR schema.

mod schema;

pub(crate) use schema::{LinkedIrStoredDocument, StoredCall, StoredMemoryObject};

use crate::Result;

pub(crate) fn parse_linked_ir(input: &str) -> Result<LinkedIrStoredDocument> {
    super::expect_identity(input, super::LINKED_IR)?;
    let document: LinkedIrStoredDocument = serde_json::from_str(input)?;
    if document.completeness_claim || document.mmio_field_semantics_claim {
        return Err(crate::Error::invalid(
            "linked-IR artifact makes an unsupported completeness or field-semantics claim",
        ));
    }
    Ok(document)
}
