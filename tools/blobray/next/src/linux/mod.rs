//! Linux adapter of in-process linking: the external ELF linker.
mod linker;
pub use linker::ElfLinker;

use blobray_domain::*;
use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
