//! Human-reviewed function names and context layouts over generated linked IR.

// The current machine IR models eight register and eight stack arguments.
// Context review must accept the complete modeled binary ABI rather than
// silently assuming every context pointer is in a0..a7.
const MAX_CONTEXT_ARGUMENTS: u8 = 16;

mod facts;
mod interface_links;
mod pack;
mod pack_parse;
mod pack_validate;
mod review;
mod template;
mod validation;

pub(crate) use facts::*;
pub(crate) use interface_links::*;
pub(crate) use pack::*;
pub(crate) use review::*;
pub(crate) use template::*;

#[cfg(test)]
mod tests;
