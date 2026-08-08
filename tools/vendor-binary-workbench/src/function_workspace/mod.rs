//! Human-reviewed function names and context layouts over generated linked IR.

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
