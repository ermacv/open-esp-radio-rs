//! Source locations from an image's DWARF: [`Symbolizer::frames`] names the
//! functions inlined at an address and where each was written. [`load`]
//! gives the debug information itself to analyses that read DWARF entries
//! (return types, variable declarations).

use crate::{Elf, Error, Result};
use std::path::Path;
use std::rc::Rc;

pub use addr2line::gimli;

/// The reader every DWARF view of this crate uses: reference-counted, so a
/// [`Symbolizer`] owns its sections.
pub type Reader = gimli::EndianRcSlice<gimli::RunTimeEndian>;

/// Debug information of one ELF.
pub type Dwarf = gimli::Dwarf<Reader>;

/// Load the debug sections of `elf`; a missing section reads empty.
pub fn load(elf: &Elf<'_>) -> Result<Dwarf> {
    use object::{Object, ObjectSection};
    let endian = if elf.object().is_little_endian() {
        gimli::RunTimeEndian::Little
    } else {
        gimli::RunTimeEndian::Big
    };
    gimli::Dwarf::load(
        |id: gimli::SectionId| -> std::result::Result<Reader, Error> {
            let data = match elf.object().section_by_name(id.name()) {
                Some(section) => section.uncompressed_data()?,
                None => Default::default(),
            };
            Ok(gimli::EndianRcSlice::new(Rc::from(&*data), endian))
        },
    )
}

/// One frame of the inline chain at an address.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Frame {
    /// The function, demangled without hashes.
    pub function: Option<String>,
    /// The source file as the compiler recorded it.
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Address-to-source lookup over one image's DWARF.
pub struct Symbolizer {
    context: addr2line::Context<Reader>,
}

impl Symbolizer {
    /// The symbolizer of the ELF `bytes`.
    pub fn new(bytes: &[u8]) -> Result<Self> {
        let dwarf = load(&Elf::parse(bytes)?)?;
        let context = addr2line::Context::from_dwarf(dwarf)
            .map_err(|error| Error::new(format!("DWARF: {error}")))?;
        Ok(Self { context })
    }

    /// The symbolizer of the ELF at `path`.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        Self::new(&bytes)
    }

    /// The frames at `address`, innermost (the inlined function) first and
    /// the out-of-line function last; empty where the DWARF describes no
    /// code.
    pub fn frames(&self, address: u64) -> Result<Vec<Frame>> {
        let error = |error: gimli::Error| Error::new(format!("DWARF at {address:#010x}: {error}"));
        let mut frames = self
            .context
            .find_frames(address)
            .skip_all_loads()
            .map_err(error)?;
        let mut out = Vec::new();
        while let Some(frame) = frames.next().map_err(error)? {
            let function = match frame.function {
                Some(function) => Some(function.demangle().map_err(error)?.into_owned()),
                None => None,
            };
            let (file, line) = match frame.location {
                Some(location) => (location.file.map(str::to_owned), location.line),
                None => (None, None),
            };
            out.push(Frame {
                function,
                file,
                line,
            });
        }
        Ok(out)
    }
}

impl From<gimli::Error> for Error {
    fn from(error: gimli::Error) -> Self {
        Error::new(format!("DWARF: {error}"))
    }
}
