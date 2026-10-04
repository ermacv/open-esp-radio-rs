//! Class-hierarchy analysis from the image's types: a call through a function
//! pointer loaded from a static reaches every function whose address the
//! image takes and whose signature can be the pointer's type.
//!
//! The pointer's type is the type of the static's field the load reads, from
//! the DWARF: the global variable at the load's address (merged and split
//! globals included) and its type's members at the load's offset, `enum` and
//! `Option` variants included, down to a pointer to a subroutine type. The
//! candidates are matched per parameter and result, never by a signature's
//! spelling: every function a relocation names other than as a call or jump
//! target (an address taken) with the subroutine type's parameter count whose
//! parameters and result no known fact contradicts. For a Rust-ABI pointer a
//! fact is a byte size or a type name (behind typedefs and qualifiers); for a
//! pointer of a foreign ABI (`extern "C"`) only a byte size, since C and Rust
//! name one type differently, and a taken function the DWARF does not
//! describe is a candidate too. An unknown size or name excludes nothing.
//!
//! A Rust-ABI pointer holds only Rust functions (another ABI's needs `unsafe`,
//! as does a transmute, which is not seen), and the soundness of excluding by
//! name rests on rustc naming one type identically in every unit and crate:
//! the debuginfo name is computed from the type's path and generic arguments
//! by one rule. A test holds the toolchain to it across crates and codegen
//! units. A site whose load address or field is unknown stays unresolved.
use crate::{Analysis, Fact, Resolutions, TransferKind};
use gimli::Reader as _;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use oer_riscv_model::{Error, ErrorCode, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

type Reader = gimli::EndianRcSlice<gimli::RunTimeEndian>;

/// One type's name and layout.
#[derive(Debug)]
struct Type {
    tag: gimli::DwTag,
    name: Option<String>,
    size: Option<u64>,
    /// Members (variants' members included) by offset, and their types.
    members: Vec<(u64, usize)>,
    /// The type a typedef, qualifier or pointer refers to.
    target: Option<usize>,
}

/// A subprogram's or subroutine type's parameter types and result type.
#[derive(Debug, Default, Clone)]
struct Signature {
    parameters: Vec<usize>,
    result: Option<usize>,
}

/// One parameter's or result's type as matching sees it: its byte size and
/// the name rustc gives it, behind typedefs and qualifiers (`None` when the
/// DWARF leaves either unknown).
type Slot = (Option<u64>, Option<String>);

/// A signature's shape: its parameters' slots and its result's.
type Shape = (Vec<Slot>, Slot);

/// Whether a function's slot can be a field's: neither a known size nor a
/// known name differs. rustc names each type once, so a type never has two
/// names; an unknown size or name contradicts nothing.
fn slot_matches(function: &Slot, field: &Slot) -> bool {
    fn agree<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }
    agree(&function.0, &field.0) && agree(&function.1, &field.1)
}

/// Whether a function's shape can be a field's; `names` compares type names
/// too, as only a Rust-ABI field may.
fn shape_matches(function: &Shape, field: &Shape, names: bool) -> bool {
    let slot = |function: &Slot, field: &Slot| {
        if names {
            slot_matches(function, field)
        } else {
            slot_matches(&(function.0, None), &(field.0, None))
        }
    };
    function.0.len() == field.0.len()
        && function.0.iter().zip(&field.0).all(|(a, b)| slot(a, b))
        && slot(&function.1, &field.1)
}

/// A function-pointer field: its subroutine type and whether its ABI is
/// foreign.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Field {
    subroutine: usize,
    foreign: bool,
}

/// What the image's DWARF states about types, globals and functions.
#[derive(Debug, Default)]
pub struct TypeFacts {
    types: BTreeMap<usize, Type>,
    /// Pieces of global variables: address, bytes (`None` for the whole
    /// variable), offset in the variable, the variable's type.
    globals: Vec<(u32, Option<u64>, u64, usize)>,
    /// Signatures of subprograms and subroutine types, by DIE.
    signatures: BTreeMap<usize, Signature>,
    /// Each function's entry and the DIE that states its signature.
    functions: BTreeMap<u32, usize>,
    /// Subprograms by their symbol: the linkage name, or the name of one
    /// without (`#[no_mangle]`). A function merged into another keeps only
    /// such an entry, without an address.
    named: BTreeMap<String, Vec<usize>>,
    /// Addresses that several functions share, with their symbols: function
    /// merging aliases a merged function to the one it keeps, with that
    /// function's size. A symbol of size zero (a linker script's or an
    /// assembly label) names no function of its own.
    merged: BTreeMap<u32, Vec<String>>,
    /// Subprograms of units that state no types at all, such as the
    /// precompiled `core` built with limited debuginfo: their parameters are
    /// unknown, not absent.
    untyped: BTreeSet<usize>,
    /// The ELF function symbols of each address, those of size zero left out.
    symbols: BTreeMap<u32, Vec<String>>,
    /// The parameter counts of each trait method, `(trait, method)`, that the
    /// typed implementations in the image state. A trait fixes its methods'
    /// parameter count whatever the implementing type.
    trait_arities: BTreeMap<(String, String), BTreeSet<usize>>,
}

/// The trait and method of a trait implementation's symbol,
/// `<Type as path::Trait>::method`, by its demangled name.
fn trait_method(symbol: &str) -> Option<(String, String)> {
    let demangled = addr2line::demangle_auto(symbol.into(), None);
    let name = crate::dwarf::plain(&demangled);
    // A legacy symbol ends in its hash.
    let name = match name.rsplit_once("::h") {
        Some((head, hash))
            if hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            head
        }
        _ => name.as_str(),
    };
    let inner = name.strip_prefix('<')?;
    let mut depth = 0_i32;
    let mut close = None;
    let mut split = None;
    for (at, character) in inner.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' if depth == 0 => {
                close = Some(at);
                break;
            }
            '>' | ')' | ']' => depth -= 1,
            ' ' if depth == 0 && inner[at..].starts_with(" as ") => split = Some(at),
            _ => {}
        }
    }
    let (close, split) = (close?, split?);
    let method = inner[close + 1..].strip_prefix("::")?;
    if method.is_empty() || method.contains(':') {
        return None;
    }
    let trait_path: String = inner[split + 4..close]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    Some((trait_path, method.to_owned()))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message.into())
}

impl TypeFacts {
    /// Read the type facts of `elf`.
    pub fn read(elf: &[u8]) -> Result<Self> {
        let file = object::File::parse(elf).map_err(|_| invalid("invalid ELF"))?;
        let dwarf = gimli::Dwarf::load(
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
        .map_err(|error| invalid(format!("DWARF: {error}")))?;
        let error = |error: gimli::Error| invalid(format!("DWARF: {error}"));
        let mut facts = Self::default();
        let mut sized: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        // Typed subprograms by their symbol, for the trait methods' arities.
        let mut typed_symbols: Vec<(usize, String)> = Vec::new();
        for symbol in file.symbols() {
            if symbol.kind() == SymbolKind::Text
                && symbol.size() > 0
                && let (Ok(address), Ok(name)) = (u32::try_from(symbol.address()), symbol.name())
            {
                sized.entry(address & !1).or_default().push(name.to_owned());
            }
        }
        facts.merged = sized
            .iter()
            .filter(|(_, symbols)| symbols.len() > 1)
            .map(|(&address, symbols)| (address, symbols.clone()))
            .collect();
        facts.symbols = sized;
        // A function's entry and the DIEs that may state its signature: its
        // own, then its abstract origin or specification.
        let mut entries: Vec<(u32, usize, Option<usize>)> = Vec::new();
        let mut units = dwarf.units();
        while let Some(header) = units.next().map_err(error)? {
            let unit = dwarf.unit(header).map_err(error)?;
            let global = |offset: gimli::UnitOffset<usize>| {
                offset
                    .to_debug_info_offset(&unit.header)
                    .map(|offset| offset.0)
            };
            let reference = |value: gimli::AttributeValue<Reader>| match value {
                gimli::AttributeValue::UnitRef(offset) => global(offset),
                gimli::AttributeValue::DebugInfoRef(offset) => Some(offset.0),
                _ => None,
            };
            // The type, subprogram or subroutine type each depth belongs to.
            let mut owners: Vec<(isize, usize, gimli::DwTag)> = Vec::new();
            // Whether the unit states any type: a unit with limited debuginfo
            // names its functions without their parameters or result.
            let mut typed = false;
            let mut subprograms = Vec::new();
            let mut unit_symbols: BTreeMap<usize, String> = BTreeMap::new();
            let mut entries_cursor = unit.entries();
            while let Some(entry) = entries_cursor.next_dfs().map_err(error)? {
                let entry = entry.clone();
                let depth = entries_cursor.depth();
                let Some(offset) = global(entries_cursor.offset()) else {
                    continue;
                };
                owners.retain(|(at, ..)| *at < depth);
                let name = entry
                    .attr_value(gimli::DW_AT_name)
                    .and_then(|value| dwarf.attr_string(&unit, value).ok())
                    .and_then(|name| name.to_string_lossy().ok().map(|name| name.into_owned()));
                let size = entry
                    .attr_value(gimli::DW_AT_byte_size)
                    .and_then(|value| value.udata_value());
                let type_of = entry.attr_value(gimli::DW_AT_type).and_then(reference);
                let tag = entry.tag();
                match tag {
                    gimli::DW_TAG_structure_type
                    | gimli::DW_TAG_union_type
                    | gimli::DW_TAG_enumeration_type
                    | gimli::DW_TAG_base_type
                    | gimli::DW_TAG_array_type
                    | gimli::DW_TAG_pointer_type
                    | gimli::DW_TAG_subroutine_type
                    | gimli::DW_TAG_typedef
                    | gimli::DW_TAG_const_type
                    | gimli::DW_TAG_volatile_type
                    | gimli::DW_TAG_restrict_type
                    | gimli::DW_TAG_atomic_type
                    | gimli::DW_TAG_reference_type
                    | gimli::DW_TAG_rvalue_reference_type
                    | gimli::DW_TAG_class_type => {
                        typed = true;
                        facts.types.insert(
                            offset,
                            Type {
                                tag,
                                name,
                                size,
                                members: Vec::new(),
                                target: type_of,
                            },
                        );
                        if tag == gimli::DW_TAG_subroutine_type {
                            facts.signatures.insert(
                                offset,
                                Signature {
                                    parameters: Vec::new(),
                                    result: type_of,
                                },
                            );
                        }
                        owners.push((depth, offset, tag));
                    }
                    gimli::DW_TAG_member => {
                        let location = match entry.attr_value(gimli::DW_AT_data_member_location) {
                            Some(value) => value.udata_value(),
                            None => Some(0),
                        };
                        // The innermost aggregate owning this member: a
                        // variant's members belong to its enum.
                        if let (Some(&(_, owner, _)), Some(location), Some(member)) = (
                            owners.iter().rev().find(|(_, _, tag)| {
                                matches!(
                                    *tag,
                                    gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type
                                )
                            }),
                            location,
                            type_of,
                        ) && let Some(owner) = facts.types.get_mut(&owner)
                        {
                            owner.members.push((location, member));
                        }
                    }
                    gimli::DW_TAG_variable => {
                        let inside_function = owners
                            .iter()
                            .any(|(_, _, tag)| *tag == gimli::DW_TAG_subprogram);
                        if let (false, Some(gimli::AttributeValue::Exprloc(expression)), Some(ty)) = (
                            inside_function,
                            entry.attr_value(gimli::DW_AT_location),
                            type_of,
                        ) {
                            facts.add_global(&expression, unit.encoding(), ty);
                        }
                    }
                    gimli::DW_TAG_subprogram => {
                        owners.push((depth, offset, tag));
                        subprograms.push(offset);
                        let symbol = entry
                            .attr_value(gimli::DW_AT_linkage_name)
                            .or_else(|| entry.attr_value(gimli::DW_AT_MIPS_linkage_name))
                            .and_then(|value| dwarf.attr_string(&unit, value).ok())
                            .and_then(|name| {
                                name.to_string_lossy().ok().map(|name| name.into_owned())
                            })
                            .or_else(|| name.clone());
                        if let Some(symbol) = symbol {
                            unit_symbols.insert(offset, symbol.clone());
                            facts.named.entry(symbol).or_default().push(offset);
                        }
                        facts.signatures.insert(
                            offset,
                            Signature {
                                parameters: Vec::new(),
                                result: type_of,
                            },
                        );
                        let low = match entry.attr_value(gimli::DW_AT_low_pc) {
                            Some(gimli::AttributeValue::Addr(low)) => u32::try_from(low).ok(),
                            _ => None,
                        };
                        let origin = entry
                            .attr_value(gimli::DW_AT_abstract_origin)
                            .or_else(|| entry.attr_value(gimli::DW_AT_specification))
                            .and_then(reference);
                        if let Some(low) = low {
                            entries.push((low, offset, origin));
                        }
                    }
                    gimli::DW_TAG_formal_parameter => {
                        typed = true;
                        if let Some(&(
                            _,
                            owner,
                            gimli::DW_TAG_subprogram | gimli::DW_TAG_subroutine_type,
                        )) = owners.last()
                            && let (Some(signature), Some(ty)) =
                                (facts.signatures.get_mut(&owner), type_of)
                        {
                            signature.parameters.push(ty);
                        }
                    }
                    _ => {}
                }
            }
            if typed {
                for offset in subprograms {
                    if let Some(symbols) = unit_symbols.remove(&offset) {
                        typed_symbols.push((offset, symbols));
                    }
                }
            } else {
                facts.untyped.extend(subprograms);
            }
        }
        for (offset, symbol) in typed_symbols {
            if let (Some(method), Some(signature)) =
                (trait_method(&symbol), facts.signatures.get(&offset))
                && !signature.parameters.is_empty()
            {
                facts
                    .trait_arities
                    .entry(method)
                    .or_default()
                    .insert(signature.parameters.len());
            }
        }
        // A concrete instance lists its parameters through its abstract
        // origin when it lists none with types of its own.
        for (low, own, origin) in entries {
            let die = match origin {
                Some(origin)
                    if facts
                        .signatures
                        .get(&own)
                        .is_none_or(|own| own.parameters.is_empty() && own.result.is_none()) =>
                {
                    origin
                }
                _ => own,
            };
            facts.functions.entry(low).or_insert(die);
        }
        Ok(facts)
    }

    /// Record a global variable's location: an address, perhaps plus an
    /// offset into a block LLVM merged globals into, perhaps in pieces.
    fn add_global(
        &mut self,
        expression: &gimli::Expression<Reader>,
        encoding: gimli::Encoding,
        ty: usize,
    ) {
        let mut operations = expression.clone().operations(encoding);
        let mut pending: Option<u32> = None;
        let mut at = 0u64;
        let mut pieces = Vec::new();
        while let Ok(Some(operation)) = operations.next() {
            match operation {
                gimli::Operation::Address { address } => pending = u32::try_from(address).ok(),
                gimli::Operation::PlusConstant { value } => {
                    pending =
                        pending.and_then(|address| address.checked_add(u32::try_from(value).ok()?));
                }
                gimli::Operation::Piece { size_in_bits, .. } => {
                    let bytes = size_in_bits / 8;
                    if let Some(address) = pending.take() {
                        pieces.push((address, Some(bytes), at));
                    }
                    at += bytes;
                }
                _ => {}
            }
        }
        if let Some(address) = pending {
            pieces.push((address, None, 0));
        }
        for (address, bytes, at) in pieces {
            self.globals.push((address, bytes, at, ty));
        }
    }

    /// The type behind typedefs and qualifiers.
    fn resolve(&self, mut offset: usize) -> Option<(usize, &Type)> {
        for _ in 0..16 {
            let ty = self.types.get(&offset)?;
            match (ty.tag, ty.target) {
                (
                    gimli::DW_TAG_typedef
                    | gimli::DW_TAG_const_type
                    | gimli::DW_TAG_volatile_type
                    | gimli::DW_TAG_restrict_type
                    | gimli::DW_TAG_atomic_type,
                    Some(target),
                ) => {
                    offset = target;
                }
                _ => return Some((offset, ty)),
            }
        }
        None
    }

    fn size(&self, offset: usize) -> Option<u64> {
        let (_, ty) = self.resolve(offset)?;
        ty.size.or(match ty.tag {
            // A C pointer or reference may omit its size: an address's.
            gimli::DW_TAG_pointer_type
            | gimli::DW_TAG_reference_type
            | gimli::DW_TAG_rvalue_reference_type => Some(4),
            // An empty tuple or struct may omit its size.
            gimli::DW_TAG_structure_type if ty.members.is_empty() => Some(0),
            _ => None,
        })
    }

    /// The function-pointer fields at byte `offset` of the type at `ty`.
    fn fields_at(&self, ty: usize, offset: u64, depth: u32, out: &mut BTreeSet<Field>) {
        if depth > 32 {
            return;
        }
        let Some((_, record)) = self.resolve(ty) else {
            return;
        };
        if offset == 0
            && record.tag == gimli::DW_TAG_pointer_type
            && let Some((subroutine, pointee)) =
                record.target.and_then(|target| self.resolve(target))
            && pointee.tag == gimli::DW_TAG_subroutine_type
        {
            let foreign = record
                .name
                .as_deref()
                .is_some_and(|name| name.contains("extern"));
            out.insert(Field {
                subroutine,
                foreign,
            });
            return;
        }
        for &(at, member) in &record.members {
            let size = self.size(member).unwrap_or(0);
            if offset >= at && (offset < at + size || (size == 0 && offset == at)) {
                self.fields_at(member, offset - at, depth + 1, out);
            }
        }
    }

    /// The function-pointer field a load from `address` reads, when exactly
    /// one global field there is one.
    fn field(&self, address: u32) -> Option<Field> {
        let mut found = BTreeSet::new();
        for &(start, bytes, at, ty) in &self.globals {
            let Some(bytes) = bytes.or_else(|| self.size(ty)) else {
                continue;
            };
            if address < start || u64::from(address - start) >= bytes {
                continue;
            }
            self.fields_at(ty, at + u64::from(address - start), 0, &mut found);
        }
        (found.len() == 1).then(|| *found.first().expect("one"))
    }

    /// A signature's shape.
    fn shape(&self, signature: &Signature) -> Shape {
        let slot = |ty: usize| -> Slot {
            let name = self.resolve(ty).and_then(|(_, ty)| ty.name.clone());
            (self.size(ty), name)
        };
        let parameters = signature.parameters.iter().map(|&ty| slot(ty)).collect();
        let result = match signature.result {
            Some(result) => slot(result),
            None => (Some(0), Some("()".into())),
        };
        (parameters, result)
    }

    /// The shape of the function entered at `entry`; `None` without a DWARF
    /// signature.
    fn function_shape(&self, entry: u32) -> Option<Shape> {
        let die = self.functions.get(&entry)?;
        Some(self.shape(self.signatures.get(die)?))
    }

    /// Whether the function at `entry` can be the field's: one of its shapes
    /// matches. Function merging puts several functions, perhaps of other
    /// types, at one address, so an address several functions share has the
    /// shape of each one's subprograms too, and a function without any is a
    /// candidate whatever the field.
    fn can_be(&self, entry: u32, wanted: &Shape, field: Field) -> bool {
        let names = !field.foreign;
        if self
            .functions
            .get(&entry)
            .is_some_and(|die| self.untyped.contains(die))
        {
            return self.untyped_can_be(entry, wanted.0.len());
        }
        let own = self.function_shape(entry);
        let Some(symbols) = self.merged.get(&entry) else {
            return match own {
                Some(shape) => shape_matches(&shape, wanted, names),
                None => field.foreign,
            };
        };
        if own.is_some_and(|shape| shape_matches(&shape, wanted, names)) {
            return true;
        }
        symbols.iter().any(|symbol| match self.named.get(symbol) {
            None => true,
            Some(dies) => dies.iter().any(|die| {
                self.untyped.contains(die)
                    || self.signatures.get(die).is_some_and(|signature| {
                        shape_matches(&self.shape(signature), wanted, names)
                    })
            }),
        })
    }

    /// Whether a function of a unit without types, whose parameters the DWARF
    /// leaves unknown, can take `arity` parameters: only a trait method's
    /// count, which the trait fixes and the image's typed implementations of
    /// the same method state, excludes it.
    fn untyped_can_be(&self, entry: u32, arity: usize) -> bool {
        let Some(symbols) = self.symbols.get(&entry) else {
            return true;
        };
        symbols.iter().any(|symbol| {
            match trait_method(symbol).and_then(|method| self.trait_arities.get(&method)) {
                Some(arities) if arities.len() == 1 => arities.contains(&arity),
                _ => true,
            }
        })
    }

    /// The candidates of a call through the field: every taken function
    /// whose shape nothing known contradicts (another parameter count, or a
    /// parameter or result of another known size or name) and, for a foreign
    /// ABI, every taken function the DWARF does not describe.
    fn candidates(&self, field: Field, taken: &BTreeSet<u32>) -> Option<BTreeSet<u32>> {
        let wanted = self.shape(self.signatures.get(&field.subroutine)?);
        Some(
            taken
                .iter()
                .copied()
                .filter(|&function| self.can_be(function, &wanted, field))
                .collect(),
        )
    }
}

/// The targets of each unresolved call through a function pointer a static's
/// field holds: the candidates among the functions `taken` lists.
pub fn function_pointer_resolutions(
    analysis: &Analysis,
    types: &TypeFacts,
    taken: &BTreeSet<u32>,
) -> Resolutions {
    let taken: BTreeSet<u32> = taken
        .iter()
        .copied()
        .filter(|address| analysis.functions.contains_key(address))
        .collect();
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            let Some(load) = transfer.load else { continue };
            let address = load.address.or_else(|| {
                let base = facts.unresolved_registers.get(&transfer.site)?[load.base as usize]?;
                Some(base.wrapping_add(load.offset as u32))
            });
            let Some(field) = address.and_then(|address| types.field(address)) else {
                continue;
            };
            if let Some(targets) = types.candidates(field, &taken) {
                resolutions.add(transfer.site, Fact::FieldType, targets);
            }
        }
    }
    resolutions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(size: Option<u64>, name: Option<&str>) -> Slot {
        (size, name.map(String::from))
    }

    #[test]
    fn a_trait_implementation_names_its_trait_and_method() {
        let method = |trait_path: &str, method: &str| Some((trait_path.into(), method.into()));
        assert_eq!(
            trait_method("<u32 as core::fmt::Display>::fmt"),
            method("core::fmt::Display", "fmt")
        );
        assert_eq!(
            trait_method("<alloc::vec::Vec<u8> as core::ops::index::Index<usize>>::index"),
            method("core::ops::index::Index<usize>", "index")
        );
        assert_eq!(
            trait_method(
                "<core::fmt::builders::PadAdapter as core::fmt::Write>::write_str::h0123456789abcdef"
            ),
            method("core::fmt::Write", "write_str")
        );
        // An inherent method or a free function names no trait.
        assert_eq!(trait_method("<core::fmt::Formatter>::pad"), None);
        assert_eq!(trait_method("core::fmt::write"), None);
    }

    #[test]
    fn only_a_known_difference_excludes_a_function() {
        let unit = || slot(Some(0), Some("()"));
        let field: Shape = (vec![slot(Some(4), Some("u32"))], unit());
        let unknown: Shape = (vec![slot(None, None)], unit());
        let unnamed: Shape = (vec![slot(Some(4), None)], unit());
        let renamed: Shape = (vec![slot(Some(4), Some("i32"))], unit());
        let resized: Shape = (vec![slot(Some(8), None)], unit());
        let arity: Shape = (vec![], unit());
        assert!(shape_matches(&field, &field, true));
        assert!(shape_matches(&unknown, &field, true));
        assert!(shape_matches(&unnamed, &field, true));
        assert!(!shape_matches(&renamed, &field, true));
        assert!(!shape_matches(&resized, &field, true));
        assert!(!shape_matches(&arity, &field, true));
        // A foreign-ABI field compares sizes only: C names `u32` otherwise.
        assert!(shape_matches(&renamed, &field, false));
        assert!(!shape_matches(&resized, &field, false));
        assert!(!shape_matches(&arity, &field, false));
    }
}
