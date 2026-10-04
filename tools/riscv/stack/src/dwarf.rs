//! Facts an image's DWARF states: the chain of functions inlined at an
//! address, and the type a function returns, by its qualified name.
use object::{Object, ObjectSection};
use oer_riscv_model::{Error, ErrorCode, Result};
use std::collections::BTreeMap;
use std::rc::Rc;

type Reader = gimli::EndianRcSlice<gimli::RunTimeEndian>;

/// The debug information of one ELF.
pub struct Dwarf {
    context: addr2line::Context<Reader>,
    /// A function's entry address and the qualified name of its return type.
    returns: BTreeMap<u32, String>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message.into())
}

fn load(elf: &[u8]) -> Result<gimli::Dwarf<Reader>> {
    let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF"))?;
    gimli::Dwarf::load(
        |id: gimli::SectionId| -> std::result::Result<Reader, gimli::Error> {
            let data = file
                .section_by_name(id.name())
                .and_then(|section| section.uncompressed_data().ok())
                .unwrap_or_default();
            Ok(gimli::EndianRcSlice::new(
                Rc::from(&*data),
                gimli::RunTimeEndian::Little,
            ))
        },
    )
    .map_err(|error| invalid(format!("DWARF: {error}")))
}

/// A demangled name without the crate disambiguators v0 symbols carry
/// (`core[1a2b]::task` reads `core::task`).
pub(crate) fn plain(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0_u32;
    for c in name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

impl Dwarf {
    /// Read the debug information of `elf`.
    pub fn read(elf: &[u8]) -> Result<Self> {
        let returns = returns(&load(elf)?)?;
        let context = addr2line::Context::from_dwarf(load(elf)?)
            .map_err(|error| invalid(format!("DWARF: {error}")))?;
        Ok(Self { context, returns })
    }

    /// The functions inlined at `address`, innermost first, by their
    /// demangled paths; empty where the DWARF covers no code.
    pub fn inline_chain(&self, address: u32) -> Result<Vec<String>> {
        let mut frames = self
            .context
            .find_frames(u64::from(address))
            .skip_all_loads()
            .map_err(|error| invalid(format!("DWARF at {address:#010x}: {error}")))?;
        let mut chain = Vec::new();
        while let Some(frame) = frames
            .next()
            .map_err(|error| invalid(format!("DWARF at {address:#010x}: {error}")))?
        {
            if let Some(function) = frame.function {
                let name = function
                    .demangle()
                    .map_err(|error| invalid(format!("DWARF at {address:#010x}: {error}")))?;
                chain.push(plain(&name));
            }
        }
        Ok(chain)
    }

    /// The linkage name, the mangled symbol, of the innermost function the
    /// DWARF places at `address`; `None` where it names none.
    pub fn innermost_symbol(&self, address: u32) -> Result<Option<String>> {
        let mut frames = self
            .context
            .find_frames(u64::from(address))
            .skip_all_loads()
            .map_err(|error| invalid(format!("DWARF at {address:#010x}: {error}")))?;
        let frame = frames
            .next()
            .map_err(|error| invalid(format!("DWARF at {address:#010x}: {error}")))?;
        Ok(frame
            .and_then(|frame| frame.function)
            .and_then(|function| function.raw_name().ok().map(|name| name.into_owned())))
    }

    /// The qualified name of the type the function entered at `function`
    /// returns, such as `core::task::wake::RawWaker`.
    pub fn return_type(&self, function: u32) -> Option<&str> {
        self.returns.get(&function).map(String::as_str)
    }
}

/// Every subprogram's entry and the qualified name of its return type.
fn returns(dwarf: &gimli::Dwarf<Reader>) -> Result<BTreeMap<u32, String>> {
    use gimli::Reader as _;
    let error = |error: gimli::Error| invalid(format!("DWARF: {error}"));
    // Qualified names of named types, by their offset in `.debug_info`.
    let mut names: BTreeMap<usize, String> = BTreeMap::new();
    // Each subprogram's entry and its return type's offset.
    let mut returned: Vec<(u32, usize)> = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next().map_err(error)? {
        let unit = dwarf.unit(header).map_err(error)?;
        let in_section = |offset: gimli::UnitOffset<usize>| {
            offset
                .to_debug_info_offset(&unit.header)
                .map(|offset| offset.0)
        };
        let mut scope: Vec<(isize, String)> = Vec::new();
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs().map_err(error)? {
            let entry = entry.clone();
            let depth = entries.depth();
            let offset = entries.offset();
            scope.retain(|(at, _)| *at < depth);
            let name: Option<String> = match entry.attr_value(gimli::DW_AT_name) {
                Some(value) => dwarf
                    .attr_string(&unit, value)
                    .ok()
                    .and_then(|name| name.to_string_lossy().ok().map(|name| name.into_owned())),
                None => None,
            };
            let qualified = |name: &str| {
                let mut path: Vec<&str> = scope.iter().map(|(_, part)| part.as_str()).collect();
                path.push(name);
                path.join("::")
            };
            match entry.tag() {
                gimli::DW_TAG_namespace => {
                    if let Some(name) = name {
                        scope.push((depth, name));
                    }
                }
                gimli::DW_TAG_structure_type
                | gimli::DW_TAG_union_type
                | gimli::DW_TAG_enumeration_type
                | gimli::DW_TAG_base_type
                | gimli::DW_TAG_typedef => {
                    if let Some(name) = name {
                        if let Some(at) = in_section(offset) {
                            names.insert(at, qualified(&name));
                        }
                        // Nested types live in the type's scope.
                        scope.push((depth, name));
                    }
                }
                gimli::DW_TAG_subprogram => {
                    let low = entry.attr_value(gimli::DW_AT_low_pc);
                    let returns = entry.attr_value(gimli::DW_AT_type);
                    if let (Some(gimli::AttributeValue::Addr(low)), Some(returns)) = (low, returns)
                    {
                        let target = match returns {
                            gimli::AttributeValue::UnitRef(offset) => in_section(offset),
                            gimli::AttributeValue::DebugInfoRef(offset) => Some(offset.0),
                            _ => None,
                        };
                        if let (Ok(low), Some(target)) = (u32::try_from(low), target) {
                            returned.push((low, target));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(returned
        .into_iter()
        .filter_map(|(low, target)| Some((low, names.get(&target)?.clone())))
        .collect())
}
