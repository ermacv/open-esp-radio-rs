//! The one recogniser of each code marker the repository's tools read:
//!
//! - [`source`]: vendor `SOURCE` citation blocks, their chips and the
//!   place a citation is attributed to (vendor provenance registers them,
//!   tidy keeps them in compiled files);
//! - [`capability`]: `// CAPABILITY:` anchor lines and the catalog ids they
//!   name (the qualification evaluator binds catalog entries through them,
//!   tidy keeps them in compiled files).
//!
//! It depends on nothing, so the gate, tidy, qualification and vendor
//! provenance share it without linking each other.
#![forbid(unsafe_code)]

pub mod capability;
pub mod source;
