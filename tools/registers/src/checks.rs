//! The register checks the gate calls (`cargo xtask check architecture`,
//! `cargo xtask register-inventory`): rules about handwritten PAC code and
//! about the published registers, owned by the register publication.
//!
//! - [`pac_transactions`]: a handwritten PAC operation is one transaction;
//! - [`shared_words`]: no MMIO word has two uncoordinated writers (radio PAC
//!   and esp-hal);
//! - [`inventory`]: the vendor's statically resolved MMIO accesses against
//!   the register model.

pub mod inventory;
pub mod pac_transactions;
pub mod shared_words;
