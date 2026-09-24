//! Streaming structural inventory over captured byte sources. No path access.
//! Archive framing and ELF inspection have one implementation each; consumers
//! choose storage or presentation through synchronous borrowed-record ports.
use blobray_domain::*;
mod definition;
mod image;
pub use definition::inspect_link_definition;
pub use image::{LinkRootFacts, ValidatedImage, inspect_link_input, validate_image};
mod cursor;
mod meter;
mod stream;
pub use cursor::{MemberCursor, MemberIndex, MemberRange};
use object::endian::Endian;
use object::read::elf::{FileHeader, Rel, Rela, SectionHeader, Sym};
use object::{Endianness, FileKind, elf};
pub use stream::{inspect_payload, inspect_source};
pub const INVENTORY_PRODUCER: &str = "blobray-artifacts/2;object/0.39.1";

fn diagnostic(
    code: DiagnosticCode,
    context: impl Into<String>,
    message: impl ToString,
) -> Diagnostic {
    Diagnostic {
        code,
        context: context.into(),
        message: message.to_string(),
    }
}

mod function;
mod mapping;
mod program;
pub use function::{DataView, FunctionView, PreparedObject, with_function, with_prepared_object};

pub use program::executable_sections;
pub use program::execution_segments;
