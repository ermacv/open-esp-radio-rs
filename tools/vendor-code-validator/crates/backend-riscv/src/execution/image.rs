//! Linked ELF image, symbol/relocation lookup and static branch inventory.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

use object::{
    Object, ObjectSection, ObjectSegment, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionKind, SymbolKind, SymbolSection,
};
use rv_asm::{Inst, Reg, Xlen};

use crate::{Result, artifact::andi_immediate};

pub(super) const RETURN_SENTINEL: u32 = 0xffff_fffc;
pub(super) const STACK_POINTER: u32 = 0x3fff_f000;
pub(super) const STACK_SIZE: u32 = 0x1_0000;

pub(super) fn execution_stack_contains(address: u32) -> bool {
    address
        .checked_sub(STACK_POINTER.wrapping_sub(STACK_SIZE))
        .is_some_and(|offset| offset < STACK_SIZE)
}

#[derive(Clone, Debug)]
pub(super) struct Segment {
    pub(super) address: u32,
    pub(super) bytes: Vec<u8>,
    pub(super) memory_size: u32,
    pub(super) writable: bool,
}

#[derive(Clone, Debug)]
pub(super) struct RelocatedCall {
    pub(super) name: String,
    pub(super) target: Option<u32>,
}

#[derive(Clone, Debug)]
pub(super) struct UnresolvedRelocation {
    pub(super) name: String,
    pub(super) r_type: u32,
    pub(super) width: u8,
}

#[derive(Clone, Debug)]
pub struct ExecutableImage {
    pub(super) segments: Vec<Segment>,
    pub(super) symbols_by_name: HashMap<String, u32>,
    pub(super) symbols_by_address: BTreeMap<u32, String>,
    /// Exact linked sizes for text symbols which carry one in the ELF symbol
    /// table. Unlike `symbols_by_address`, this deliberately excludes
    /// absolute call targets and zero-sized labels.
    pub(super) symbol_sizes_by_address: BTreeMap<u32, u32>,
    /// Text definitions with local ELF binding. These form the implementation
    /// closure of a selected public probe; calls to another global definition
    /// remain named ABI boundaries even when that definition is linked into
    /// the same diagnostic image.
    pub(super) local_text_symbols: BTreeSet<u32>,
    pub(super) call_trampoline_addresses: BTreeSet<u32>,
    pub(super) relocated_calls_by_address: BTreeMap<u32, RelocatedCall>,
    /// Allocated bytes whose linked value still depends on an undefined
    /// symbol. Keeping these as poison lets an unrelated function in a large
    /// linked oracle run while any reachable use still fails closed.
    pub(super) unresolved_relocations_by_address: BTreeMap<u32, UnresolvedRelocation>,
    pub(super) global_pointer: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CoverageInventory {
    pub branch_sites: BTreeSet<u32>,
    pub branch_outcomes: BTreeSet<(u32, bool)>,
    pub unresolved_edges: BTreeMap<u32, String>,
}

impl ExecutableImage {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;
        let file = object::File::parse(bytes.as_slice())?;
        if file.architecture() != object::Architecture::Riscv32 || !file.is_little_endian() {
            return Err("execution requires a little-endian RISC-V 32-bit ELF".into());
        }
        let mut segments = Vec::new();
        for segment in file.segments() {
            if segment.address() == 0 || segment.size() == 0 {
                continue;
            }
            let address = u32::try_from(segment.address())
                .map_err(|_| "load segment address does not fit RV32")?;
            let memory_size =
                u32::try_from(segment.size()).map_err(|_| "load segment size does not fit RV32")?;
            let data = segment.data()?;
            if data.len() > memory_size as usize {
                return Err("ELF segment file size exceeds its memory size".into());
            }
            segments.push(Segment {
                address,
                bytes: data.to_vec(),
                memory_size,
                writable: matches!(
                    segment.flags(),
                    object::SegmentFlags::Elf { p_flags }
                        if p_flags & object::elf::PF_W != 0
                ),
            });
        }
        segments.sort_by_key(|segment| segment.address);

        let mut symbols_by_name = HashMap::new();
        let mut symbols_by_address = BTreeMap::new();
        let mut symbol_sizes_by_address: BTreeMap<u32, u32> = BTreeMap::new();
        let mut local_text_symbols = BTreeSet::new();
        let mut call_trampoline_addresses = BTreeSet::new();
        let mut global_pointer = None;
        for symbol in file.symbols() {
            if !symbol.is_definition() || symbol.address() == 0 {
                continue;
            }
            let Ok(name) = symbol.name() else {
                continue;
            };
            let address = symbol.address() as u32;
            if name.starts_with("__call_") {
                call_trampoline_addresses.insert(address);
            }
            let absolute_callable =
                symbol.kind() == SymbolKind::Unknown && symbol.section() == SymbolSection::Absolute;
            if symbol.kind() == SymbolKind::Text
                || symbol.kind() == SymbolKind::Data
                || absolute_callable
            {
                symbols_by_name.insert(name.to_owned(), address);
            }
            if symbol.kind() == SymbolKind::Text || absolute_callable {
                symbols_by_address
                    .entry(address)
                    .or_insert_with(|| name.to_owned());
            }
            if symbol.kind() == SymbolKind::Text
                && let Ok(size) = u32::try_from(symbol.size())
                && size != 0
            {
                symbol_sizes_by_address
                    .entry(address)
                    .and_modify(|current| *current = (*current).max(size))
                    .or_insert(size);
                if !symbol.is_global() && !symbol.is_weak() {
                    local_text_symbols.insert(address);
                }
            }
            if name == "__global_pointer$" {
                global_pointer = Some(address);
            }
        }
        let mut relocated_calls_by_address = BTreeMap::new();
        let mut unresolved_relocations_by_address = BTreeMap::new();
        for section in file.sections() {
            for (offset, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type } = relocation.flags() else {
                    continue;
                };
                let section_start = section.address();
                let section_end = section_start.wrapping_add(section.size());
                let address = if offset >= section_start && offset < section_end {
                    offset
                } else {
                    section_start.wrapping_add(offset)
                } as u32;
                if !matches!(
                    r_type,
                    object::elf::R_RISCV_CALL | object::elf::R_RISCV_CALL_PLT
                ) {
                    if !matches!(
                        r_type,
                        object::elf::R_RISCV_NONE | object::elf::R_RISCV_RELAX
                    ) && matches!(
                        section.kind(),
                        SectionKind::Text
                            | SectionKind::ReadOnlyData
                            | SectionKind::ReadOnlyString
                            | SectionKind::Data
                            | SectionKind::Tls
                    ) && let RelocationTarget::Symbol(index) = relocation.target()
                    {
                        let symbol = file.symbol_by_index(index)?;
                        if !symbol.is_definition() && symbol.section() != SymbolSection::Absolute {
                            unresolved_relocations_by_address.insert(
                                address,
                                UnresolvedRelocation {
                                    name: symbol.name().unwrap_or("<unnamed>").to_owned(),
                                    r_type,
                                    width: unresolved_relocation_width(
                                        r_type,
                                        relocation.size(),
                                        section.kind() == SectionKind::Text,
                                    ),
                                },
                            );
                        }
                    }
                    continue;
                }
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    continue;
                };
                let symbol = file.symbol_by_index(index)?;
                let name = symbol.name()?.to_owned();
                let target = (symbol.is_definition()
                    || symbol.section() == SymbolSection::Absolute)
                    .then_some(symbol.address() as u32)
                    .filter(|address| *address != 0);
                relocated_calls_by_address.insert(address, RelocatedCall { name, target });
            }
        }

        Ok(Self {
            segments,
            symbols_by_name,
            symbols_by_address,
            symbol_sizes_by_address,
            local_text_symbols,
            call_trampoline_addresses,
            relocated_calls_by_address,
            unresolved_relocations_by_address,
            global_pointer,
        })
    }

    pub fn add_companion(&mut self, path: &Path) -> Result<()> {
        let companion = Self::load(path)?;
        self.segments.extend(companion.segments);
        self.segments.sort_by_key(|segment| segment.address);
        for (name, address) in companion.symbols_by_name {
            self.symbols_by_name.entry(name).or_insert(address);
        }
        for (address, name) in companion.symbols_by_address {
            self.symbols_by_address.entry(address).or_insert(name);
        }
        for (address, size) in companion.symbol_sizes_by_address {
            self.symbol_sizes_by_address
                .entry(address)
                .and_modify(|current| *current = (*current).max(size))
                .or_insert(size);
        }
        self.local_text_symbols.extend(companion.local_text_symbols);
        self.call_trampoline_addresses
            .extend(companion.call_trampoline_addresses);
        for (address, call) in companion.relocated_calls_by_address {
            self.relocated_calls_by_address
                .entry(address)
                .or_insert(call);
        }
        for (address, relocation) in companion.unresolved_relocations_by_address {
            self.unresolved_relocations_by_address
                .entry(address)
                .or_insert(relocation);
        }
        self.resolve_external_relocations();
        Ok(())
    }

    pub(super) fn resolve_external_relocations(&mut self) {
        for call in self.relocated_calls_by_address.values_mut() {
            if call.target.is_none() {
                call.target = self.symbols_by_name.get(&call.name).copied();
            }
        }
    }

    pub fn symbol_address(&self, name: &str) -> Option<u32> {
        self.symbols_by_name.get(name).copied()
    }

    /// Return the half-open text extent of one linked symbol.
    ///
    /// The validator uses this only to distinguish calls issued directly by
    /// an architectural root from calls made by its children. Linked ELF
    /// symbols are address ordered, so the next text symbol is the fail-closed
    /// end boundary even when the input symbol table omits explicit sizes.
    pub fn symbol_extent(&self, name: &str) -> Option<std::ops::Range<u32>> {
        let start = self.symbol_address(name)?;
        if let Some(size) = self.symbol_sizes_by_address.get(&start) {
            return start.checked_add(*size).map(|end| start..end);
        }
        let end = self
            .symbols_by_address
            .range((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .next()
            .map(|(address, _)| *address)?;
        Some(start..end)
    }

    /// Canonicalize the linked code closure rooted at one exact text symbol.
    ///
    /// The identity is deliberately independent of the surrounding ELF and
    /// of linked addresses. Exact function bytes are retained, except for
    /// direct inter-function call encodings: those are represented as stable
    /// edges to recursively canonicalized local callees. Calls to another
    /// global symbol remain named ABI edges. This makes qualification evidence
    /// local to the selected probe while still binding compiler/internal
    /// helpers which were not inlined.
    ///
    /// Direct `JAL` and adjacent `AUIPC`/`JALR` edges are followed. A symbolic
    /// ELF call relocation is retained by name. Indirect calls remain part of
    /// the caller bytes and therefore still require execution scenarios or an
    /// explicit ABI adapter to qualify their possible targets.
    pub fn code_closure_identity(&self, root_symbol: &str) -> Result<String> {
        const MAX_FUNCTIONS: usize = 4_096;
        const MAX_INSTRUCTIONS: usize = 1_000_000;

        let root = self
            .symbol_address(root_symbol)
            .ok_or_else(|| format!("execution symbol {root_symbol} was not found"))?;
        if !self.symbol_sizes_by_address.contains_key(&root) {
            return Err(
                format!("execution symbol {root_symbol} has no exact linked text size").into(),
            );
        }

        let mut addresses = vec![root];
        let mut indices = BTreeMap::from([(root, 0_usize)]);
        let mut canonical = String::from("riscv32-code-closure-v1\n");
        let mut instruction_count = 0_usize;
        let mut node = 0_usize;

        while node < addresses.len() {
            if addresses.len() > MAX_FUNCTIONS {
                return Err(format!(
                    "code closure for {root_symbol} exceeds {MAX_FUNCTIONS} functions"
                )
                .into());
            }
            let start = addresses[node];
            let size = self.symbol_sizes_by_address[&start];
            let end = start
                .checked_add(size)
                .ok_or("linked text symbol extent overflows RV32 address space")?;
            canonical.push_str(&format!("node {node} size={size}\n"));

            let mut address = start;
            while address < end {
                instruction_count += 1;
                if instruction_count > MAX_INSTRUCTIONS {
                    return Err(format!(
                        "code closure for {root_symbol} exceeds {MAX_INSTRUCTIONS} instructions"
                    )
                    .into());
                }
                let offset = address - start;

                if let Some((relocation_site, relocation)) = self.unresolved_relocation_at(address)
                {
                    if relocation_site < start {
                        return Err(format!(
                            "linked symbol at {start:#x} begins inside unresolved relocation at {relocation_site:#x}"
                        )
                        .into());
                    }
                    let relocation_offset = relocation_site - start;
                    canonical.push_str(&format!(
                        "unresolved-relocation +{relocation_offset:#x} type={} width={} symbol={}\n",
                        relocation.r_type, relocation.width, relocation.name
                    ));
                    let relocation_end =
                        relocation_site
                            .checked_add(u32::from(relocation.width))
                            .ok_or("unresolved relocation extent overflows RV32 address space")?;
                    if relocation_end > end {
                        return Err(format!(
                            "unresolved relocation at {address:#x} crosses linked symbol extent"
                        )
                        .into());
                    }
                    address = relocation_end;
                    continue;
                }

                if let Some(call) = self.relocated_call_at(address) {
                    let target_node = call
                        .target
                        .filter(|target| self.closure_owns(root, *target))
                        .map(|target| closure_node(target, &mut addresses, &mut indices));
                    canonical.push_str(&format!(
                        "reloc-call +{offset:#x} symbol={} target={}\n",
                        call.name,
                        target_node
                            .map_or_else(|| "external".to_owned(), |index| index.to_string())
                    ));
                    let pair_end = address
                        .checked_add(8)
                        .ok_or("R_RISCV_CALL pair overflows RV32 address space")?;
                    if pair_end > end {
                        return Err(format!(
                            "R_RISCV_CALL at {address:#x} exceeds linked symbol extent"
                        )
                        .into());
                    }
                    // Validate the pair even though its address-bearing bytes
                    // are intentionally excluded from the identity.
                    self.relocated_call_link_register(address)?;
                    address = pair_end;
                    continue;
                }

                let Ok((instruction, width)) = self.instruction(address) else {
                    // Rust and vendor linkers may place jump tables or literal
                    // data inside a FUNC-sized range. The exact remainder is
                    // still part of this local implementation identity, but
                    // it cannot safely be interpreted as more call edges.
                    canonical.push_str(&format!("opaque-bytes +{offset:#x} "));
                    for byte_address in address..end {
                        let byte = self.byte(byte_address).ok_or_else(|| {
                            format!("linked symbol byte is absent at {byte_address:#x}")
                        })?;
                        canonical.push_str(&format!("{byte:02x}"));
                    }
                    canonical.push('\n');
                    break;
                };
                let next = address
                    .checked_add(width)
                    .ok_or("instruction extent overflows RV32 address space")?;
                if next > end {
                    return Err(format!(
                        "instruction at {address:#x} crosses linked symbol extent"
                    )
                    .into());
                }

                if let Inst::Jal { offset: jump, dest } = instruction {
                    let target = address.wrapping_add(jump.as_u32());
                    if !(start..end).contains(&target) {
                        write_closure_edge(
                            &mut canonical,
                            "jal",
                            offset,
                            dest,
                            target,
                            root,
                            self,
                            &mut addresses,
                            &mut indices,
                        );
                        address = next;
                        continue;
                    }
                }

                if let Inst::Auipc { uimm, dest: base } = instruction
                    && next < end
                {
                    let (following, following_width) = self.instruction(next)?;
                    if let Inst::Jalr {
                        offset: jump,
                        base: jump_base,
                        dest,
                    } = following
                        && jump_base == base
                    {
                        let pair_end = next
                            .checked_add(following_width)
                            .ok_or("AUIPC/JALR extent overflows RV32 address space")?;
                        if pair_end > end {
                            return Err(format!(
                                "AUIPC/JALR at {address:#x} crosses linked symbol extent"
                            )
                            .into());
                        }
                        let target = address
                            .wrapping_add(uimm.as_u32())
                            .wrapping_add(jump.as_u32())
                            & !1;
                        write_closure_edge(
                            &mut canonical,
                            "auipc-jalr",
                            offset,
                            dest,
                            target,
                            root,
                            self,
                            &mut addresses,
                            &mut indices,
                        );
                        address = pair_end;
                        continue;
                    }
                }

                canonical.push_str(&format!("bytes +{offset:#x} "));
                for byte_address in address..next {
                    let byte = self.byte(byte_address).ok_or_else(|| {
                        format!("linked symbol byte is absent at {byte_address:#x}")
                    })?;
                    canonical.push_str(&format!("{byte:02x}"));
                }
                canonical.push('\n');
                address = next;
            }
            node += 1;
        }

        Ok(canonical)
    }

    fn closure_owns(&self, root: u32, address: u32) -> bool {
        address == root
            || (self.symbol_sizes_by_address.contains_key(&address)
                && self.local_text_symbols.contains(&address))
    }

    pub(super) fn symbol_at(&self, address: u32) -> Option<&str> {
        self.symbols_by_address.get(&address).map(String::as_str)
    }

    pub(super) fn relocated_call_at(&self, address: u32) -> Option<&RelocatedCall> {
        self.relocated_calls_by_address.get(&address)
    }

    pub(super) fn unresolved_relocation_at(
        &self,
        address: u32,
    ) -> Option<(u32, &UnresolvedRelocation)> {
        // RISC-V relocations in allocated sections are at most eight bytes in
        // the supported RV32 oracle format. Searching the preceding seven
        // sites also detects a read/fetch into the middle of a relocated word.
        self.unresolved_relocations_by_address
            .range(address.saturating_sub(7)..=address)
            .rev()
            .find_map(|(start, relocation)| {
                address
                    .checked_sub(*start)
                    .is_some_and(|offset| offset < u32::from(relocation.width))
                    .then_some((*start, relocation))
            })
    }

    pub(super) fn unresolved_relocation_error(
        &self,
        address: u32,
        operation: &str,
    ) -> Option<String> {
        self.unresolved_relocation_at(address)
            .map(|(site, relocation)| {
                format!(
                    "{operation} reached unresolved ELF relocation type {} to {} at {site:#010x}",
                    relocation.r_type, relocation.name
                )
            })
    }

    pub fn relocated_calls(&self) -> BTreeMap<u32, (String, Option<u32>)> {
        self.relocated_calls_by_address
            .iter()
            .map(|(address, call)| (*address, (call.name.clone(), call.target)))
            .collect()
    }

    pub(super) fn relocated_call_link_register(&self, address: u32) -> Result<Reg> {
        match self.instruction(address.wrapping_add(4))?.0 {
            Inst::Jalr { dest, .. } => Ok(dest),
            instruction => Err(format!(
                "R_RISCV_CALL at {address:#x} is not followed by JALR: {instruction}"
            )
            .into()),
        }
    }

    pub fn location(&self, address: u32) -> String {
        self.symbols_by_address
            .range(..=address)
            .next_back()
            .map_or_else(
                || format!("{address:#010x}"),
                |(start, symbol)| format!("{symbol}+{:#x}", address.wrapping_sub(*start)),
            )
    }

    /// Finds every conditional branch reachable through direct control flow.
    ///
    /// Both feasible successors of a conditional branch are explored. With no
    /// argument constraints both remain feasible; a concrete ABI fact prunes
    /// the complete unreachable descendant graph. Direct calls are followed
    /// as well as their return continuation, so branch coverage also includes
    /// statically linked children. An indirect edge is retained as unresolved
    /// only when propagated constants cannot determine its target.
    pub fn coverage_inventory(&self, symbol: &str) -> Result<CoverageInventory> {
        self.coverage_inventory_with_argument_constraints(symbol, &[None; 8])
    }

    pub fn coverage_inventory_with_arguments(
        &self,
        symbol: &str,
        arguments: Option<&[u32; 8]>,
    ) -> Result<CoverageInventory> {
        let constraints = arguments.map_or([None; 8], |arguments| {
            core::array::from_fn(|index| Some(arguments[index]))
        });
        self.coverage_inventory_with_argument_constraints(symbol, &constraints)
    }

    /// Inventories control flow reachable under partial ABI argument facts.
    ///
    /// `None` leaves one `a0..a7` input unconstrained. A concrete value prunes
    /// both conditional outcomes and all descendants of an infeasible edge,
    /// including panic paths and their non-returning fallthrough bytes.
    pub fn coverage_inventory_with_argument_constraints(
        &self,
        symbol: &str,
        arguments: &[Option<u32>; 8],
    ) -> Result<CoverageInventory> {
        let start = self
            .symbol_address(symbol)
            .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
        const MAX_ABSTRACT_STATES: usize = 200_000;

        let mut initial = [None; 32];
        initial[usize::from(Reg::ZERO.0)] = Some(0);
        for (index, value) in arguments.iter().copied().enumerate() {
            initial[usize::from(Reg::A0.0) + index] = value;
        }
        let unknown_after_call = || {
            let mut registers = [None; 32];
            registers[usize::from(Reg::ZERO.0)] = Some(0);
            registers
        };
        let mut pending = vec![(start, initial)];
        let mut visited = BTreeSet::new();
        let mut inventory = CoverageInventory::default();

        while let Some((address, mut registers)) = pending.pop() {
            if !visited.insert((address, registers)) {
                continue;
            }
            if visited.len() > MAX_ABSTRACT_STATES {
                return Err(format!(
                    "abstract branch analysis exceeded {MAX_ABSTRACT_STATES} states"
                )
                .into());
            }
            if self.symbol_at(address) == Some("ets_delay_us") {
                continue;
            }
            if let Some(call) = self.relocated_call_at(address) {
                let link = self.relocated_call_link_register(address)?;
                if link != Reg::ZERO {
                    pending.push((address.wrapping_add(8), unknown_after_call()));
                }
                if let Some(target) = call.target
                    && self.symbol_at(target) != Some("ets_delay_us")
                {
                    pending.push((target, registers));
                } else if call.target.is_none() {
                    inventory
                        .unresolved_edges
                        .insert(address, format!("external-call {}", call.name));
                }
                continue;
            }
            let (instruction, width) = self.instruction(address)?;
            let next = address.wrapping_add(width);
            let get =
                |registers: &[Option<u32>; 32], register: Reg| registers[usize::from(register.0)];
            let set = |registers: &mut [Option<u32>; 32], register: Reg, value: Option<u32>| {
                if register != Reg::ZERO {
                    registers[usize::from(register.0)] = value;
                }
            };
            match instruction {
                Inst::Lui { uimm, dest } => {
                    set(&mut registers, dest, Some(uimm.as_u32()));
                    pending.push((next, registers));
                }
                Inst::Auipc { uimm, dest } => {
                    set(
                        &mut registers,
                        dest,
                        Some(address.wrapping_add(uimm.as_u32())),
                    );
                    pending.push((next, registers));
                }
                Inst::Addi { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value.wrapping_add(imm.as_u32()));
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Andi { imm, dest, src1 } => {
                    let immediate = andi_immediate(imm, width as u8);
                    let value = get(&registers, src1).map(|value| value & immediate);
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Ori { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value | imm.as_u32());
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Xori { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value ^ imm.as_u32());
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Slli { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value << (imm.as_u32() & 31));
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Srli { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value >> (imm.as_u32() & 31));
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Srai { imm, dest, src1 } => {
                    let value = get(&registers, src1)
                        .map(|value| ((value as i32) >> (imm.as_u32() & 31)) as u32);
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Slti { imm, dest, src1 } => {
                    let value =
                        get(&registers, src1).map(|value| u32::from((value as i32) < imm.as_i32()));
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Sltiu { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| u32::from(value < imm.as_u32()));
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Add { dest, src1, src2 }
                | Inst::Sub { dest, src1, src2 }
                | Inst::And { dest, src1, src2 }
                | Inst::Or { dest, src1, src2 }
                | Inst::Xor { dest, src1, src2 }
                | Inst::Sll { dest, src1, src2 }
                | Inst::Srl { dest, src1, src2 }
                | Inst::Sra { dest, src1, src2 }
                | Inst::Slt { dest, src1, src2 }
                | Inst::Sltu { dest, src1, src2 }
                | Inst::Mul { dest, src1, src2 }
                | Inst::Div { dest, src1, src2 }
                | Inst::Divu { dest, src1, src2 }
                | Inst::Rem { dest, src1, src2 }
                | Inst::Remu { dest, src1, src2 } => {
                    let left = get(&registers, src1);
                    let right = get(&registers, src2);
                    let value = left.zip(right).map(|(left, right)| match instruction {
                        Inst::Add { .. } => left.wrapping_add(right),
                        Inst::Sub { .. } => left.wrapping_sub(right),
                        Inst::And { .. } => left & right,
                        Inst::Or { .. } => left | right,
                        Inst::Xor { .. } => left ^ right,
                        Inst::Sll { .. } => left << (right & 31),
                        Inst::Srl { .. } => left >> (right & 31),
                        Inst::Sra { .. } => ((left as i32) >> (right & 31)) as u32,
                        Inst::Slt { .. } => u32::from((left as i32) < (right as i32)),
                        Inst::Sltu { .. } => u32::from(left < right),
                        Inst::Mul { .. } => left.wrapping_mul(right),
                        Inst::Div { .. } => {
                            let (left, right) = (left as i32, right as i32);
                            if right == 0 {
                                u32::MAX
                            } else if left == i32::MIN && right == -1 {
                                i32::MIN as u32
                            } else {
                                (left / right) as u32
                            }
                        }
                        Inst::Divu { .. } => left.checked_div(right).unwrap_or(u32::MAX),
                        Inst::Rem { .. } => {
                            let (left, right) = (left as i32, right as i32);
                            if right == 0 {
                                left as u32
                            } else if left == i32::MIN && right == -1 {
                                0
                            } else {
                                (left % right) as u32
                            }
                        }
                        Inst::Remu { .. } => {
                            if right == 0 {
                                left
                            } else {
                                left % right
                            }
                        }
                        _ => unreachable!(),
                    });
                    set(&mut registers, dest, value);
                    pending.push((next, registers));
                }
                Inst::Mulh { dest, .. }
                | Inst::Mulhsu { dest, .. }
                | Inst::Mulhu { dest, .. }
                | Inst::Lb { dest, .. }
                | Inst::Lbu { dest, .. }
                | Inst::Lh { dest, .. }
                | Inst::Lhu { dest, .. }
                | Inst::Lw { dest, .. } => {
                    set(&mut registers, dest, None);
                    pending.push((next, registers));
                }
                Inst::Sb { .. } | Inst::Sh { .. } | Inst::Sw { .. } | Inst::Fence { .. } => {
                    pending.push((next, registers));
                }
                Inst::Beq { offset, src1, src2 }
                | Inst::Bne { offset, src1, src2 }
                | Inst::Blt { offset, src1, src2 }
                | Inst::Bge { offset, src1, src2 }
                | Inst::Bltu { offset, src1, src2 }
                | Inst::Bgeu { offset, src1, src2 } => {
                    inventory.branch_sites.insert(address);
                    let known = get(&registers, src1).zip(get(&registers, src2));
                    let taken = known.map(|(left, right)| match instruction {
                        Inst::Beq { .. } => left == right,
                        Inst::Bne { .. } => left != right,
                        Inst::Blt { .. } => (left as i32) < (right as i32),
                        Inst::Bge { .. } => (left as i32) >= (right as i32),
                        Inst::Bltu { .. } => left < right,
                        Inst::Bgeu { .. } => left >= right,
                        _ => unreachable!(),
                    });
                    for outcome in
                        taken.map_or([Some(false), Some(true)], |taken| [Some(taken), None])
                    {
                        let Some(outcome) = outcome else {
                            continue;
                        };
                        inventory.branch_outcomes.insert((address, outcome));
                        pending.push((
                            if outcome {
                                address.wrapping_add(offset.as_u32())
                            } else {
                                next
                            },
                            registers,
                        ));
                    }
                }
                Inst::Jal { offset, dest } => {
                    let target = address.wrapping_add(offset.as_u32());
                    if dest == Reg::ZERO {
                        pending.push((target, registers));
                    } else {
                        set(&mut registers, dest, Some(next));
                        pending.push((target, registers));
                        pending.push((next, unknown_after_call()));
                    }
                }
                Inst::Jalr { offset, base, dest }
                    if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 => {}
                Inst::Jalr { offset, base, dest } => {
                    if let Some(base) = get(&registers, base) {
                        let target = base.wrapping_add(offset.as_u32()) & !1;
                        if dest == Reg::ZERO {
                            pending.push((target, registers));
                        } else {
                            set(&mut registers, dest, Some(next));
                            pending.push((target, registers));
                            pending.push((next, unknown_after_call()));
                        }
                    } else {
                        if dest != Reg::ZERO {
                            pending.push((next, unknown_after_call()));
                        }
                        inventory
                            .unresolved_edges
                            .insert(address, instruction.to_string());
                    }
                }
                Inst::Ecall | Inst::Ebreak => {}
                _ => {
                    // An unsupported instruction may define any register.
                    // Forget all constants so later branches remain
                    // conservative, then continue through the fallthrough.
                    pending.push((next, unknown_after_call()));
                }
            }
        }
        Ok(inventory)
    }

    pub(super) fn byte(&self, address: u32) -> Option<u8> {
        if self.unresolved_relocation_at(address).is_some() {
            return None;
        }
        self.segments.iter().find_map(|segment| {
            let offset = address.checked_sub(segment.address)? as usize;
            if offset >= segment.memory_size as usize {
                None
            } else {
                Some(segment.bytes.get(offset).copied().unwrap_or(0))
            }
        })
    }

    /// Returns one byte from the linked ELF load image, including the
    /// zero-filled `p_memsz - p_filesz` tail used for BSS.
    pub fn loaded_byte(&self, address: u32) -> Option<u8> {
        self.byte(address)
    }

    pub(super) fn contains_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            address
                .checked_sub(segment.address)
                .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    pub(super) fn contains_writable_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            segment.writable
                && address
                    .checked_sub(segment.address)
                    .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    pub(super) fn instruction(&self, address: u32) -> Result<(Inst, u32)> {
        if let Some(error) = self.unresolved_relocation_error(address, "instruction fetch") {
            return Err(error.into());
        }
        let low = self
            .byte(address)
            .ok_or_else(|| format!("instruction fetch outside image at {address:#x}"))?;
        let width = if Inst::first_byte_is_compressed(low) {
            2
        } else {
            4
        };
        let mut word = [0_u8; 4];
        for (offset, byte) in word.iter_mut().take(width as usize).enumerate() {
            let byte_address = address.wrapping_add(offset as u32);
            if let Some(error) = self.unresolved_relocation_error(byte_address, "instruction fetch")
            {
                return Err(error.into());
            }
            *byte = self
                .byte(byte_address)
                .ok_or_else(|| format!("truncated instruction at {address:#x}"))?;
        }
        let (instruction, _) = Inst::decode(u32::from_le_bytes(word), Xlen::Rv32)
            .map_err(|error| format!("cannot decode instruction at {address:#x}: {error}"))?;
        Ok((instruction, width))
    }
}

fn closure_node(
    address: u32,
    addresses: &mut Vec<u32>,
    indices: &mut BTreeMap<u32, usize>,
) -> usize {
    if let Some(index) = indices.get(&address) {
        return *index;
    }
    let index = addresses.len();
    addresses.push(address);
    indices.insert(address, index);
    index
}

#[allow(
    clippy::too_many_arguments,
    reason = "closure edge rendering keeps traversal state explicit and architecture-local"
)]
fn write_closure_edge(
    canonical: &mut String,
    encoding: &str,
    offset: u32,
    destination: Reg,
    target: u32,
    root: u32,
    image: &ExecutableImage,
    addresses: &mut Vec<u32>,
    indices: &mut BTreeMap<u32, usize>,
) {
    if image.closure_owns(root, target) {
        let target_node = closure_node(target, addresses, indices);
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} target={target_node}\n",
            destination.0
        ));
    } else if let Some(symbol) = image.symbol_at(target) {
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} external-symbol={symbol}\n",
            destination.0
        ));
    } else {
        canonical.push_str(&format!(
            "edge {encoding} +{offset:#x} dest={} external-address={target:#010x}\n",
            destination.0
        ));
    }
}

fn unresolved_relocation_width(r_type: u32, reported_bits: u8, executable_text: bool) -> u8 {
    // Espressif RV32 vendor objects use R_RISCV_64 on a four-byte instruction
    // image for unresolved pointer materialization. The relocation name still
    // describes the source C type; it does not make an RV32 instruction eight
    // bytes wide.
    if executable_text && r_type == object::elf::R_RISCV_64 {
        return 4;
    }
    let reported_bytes = reported_bits.div_ceil(8);
    if reported_bytes != 0 {
        return reported_bytes.min(8);
    }
    match r_type {
        object::elf::R_RISCV_64 => 8,
        object::elf::R_RISCV_RVC_BRANCH | object::elf::R_RISCV_RVC_JUMP => 2,
        // One poisoned byte is sufficient to reject a use of an unfamiliar
        // relocation without guessing its encoding. Known RV32 instruction
        // and pointer relocations occupy four bytes.
        object::elf::R_RISCV_32
        | object::elf::R_RISCV_RELATIVE
        | object::elf::R_RISCV_HI20
        | object::elf::R_RISCV_LO12_I
        | object::elf::R_RISCV_LO12_S
        | object::elf::R_RISCV_PCREL_HI20
        | object::elf::R_RISCV_PCREL_LO12_I
        | object::elf::R_RISCV_PCREL_LO12_S
        | object::elf::R_RISCV_GOT_HI20 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rv32_vendor_text_does_not_treat_one_instruction_as_an_eight_byte_pointer() {
        assert_eq!(
            unresolved_relocation_width(object::elf::R_RISCV_64, 64, true),
            4
        );
        assert_eq!(
            unresolved_relocation_width(object::elf::R_RISCV_64, 64, false),
            8
        );
    }
}
