//! Applicability of temporary executable reconstructions to a linked ROM body.
//!
//! Digests were extracted with `artifact::load_code_symbol_exact` after checking
//! the whole ROM SHA-256 against the reviewed register evidence in
//! `registers/evidence/vendor-rom.toml`:
//! `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
//! They identify reviewed bytes; they do not establish caller preconditions or
//! authenticate any callee implementation outside the selected body.

use sha2::{Digest, Sha256};

use crate::artifact::ArtifactSymbolDefinition;

pub(super) struct ReviewedLinkedBody {
    pub(super) name: &'static str,
    pub(super) address: u64,
    pub(super) size: usize,
    pub(super) sha256: &'static str,
}

impl ReviewedLinkedBody {
    pub(super) fn matches(&self, symbol: &ArtifactSymbolDefinition) -> bool {
        symbol.name == self.name
            && symbol.address == self.address
            && symbol.bytes.len() == self.size
            && symbol.addresses_resolved
            && symbol.member.is_none()
            && symbol.relocations.is_empty()
            && format!("{:x}", Sha256::digest(&symbol.bytes)) == self.sha256
    }
}
