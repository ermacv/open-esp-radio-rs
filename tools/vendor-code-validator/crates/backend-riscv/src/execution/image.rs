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
pub struct ExecutableImage {
    pub(super) segments: Vec<Segment>,
    pub(super) symbols_by_name: HashMap<String, u32>,
    pub(super) symbols_by_address: BTreeMap<u32, String>,
    pub(super) call_trampoline_addresses: BTreeSet<u32>,
    pub(super) relocated_calls_by_address: BTreeMap<u32, RelocatedCall>,
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
            if name == "__global_pointer$" {
                global_pointer = Some(address);
            }
        }
        let mut relocated_calls_by_address = BTreeMap::new();
        for section in file.sections() {
            for (offset, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type } = relocation.flags() else {
                    continue;
                };
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
                            return Err(format!(
                                "unresolved alloc-section relocation type {r_type} to {}",
                                symbol.name().unwrap_or("<unnamed>")
                            )
                            .into());
                        }
                    }
                    continue;
                }
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    continue;
                };
                let symbol = file.symbol_by_index(index)?;
                let name = symbol.name()?.to_owned();
                let section_start = section.address();
                let section_end = section_start.wrapping_add(section.size());
                let address = if offset >= section_start && offset < section_end {
                    offset
                } else {
                    section_start.wrapping_add(offset)
                } as u32;
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
            call_trampoline_addresses,
            relocated_calls_by_address,
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
        self.call_trampoline_addresses
            .extend(companion.call_trampoline_addresses);
        for (address, call) in companion.relocated_calls_by_address {
            self.relocated_calls_by_address
                .entry(address)
                .or_insert(call);
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
        let end = self
            .symbols_by_address
            .range((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .next()
            .map(|(address, _)| *address)?;
        Some(start..end)
    }

    pub(super) fn symbol_at(&self, address: u32) -> Option<&str> {
        self.symbols_by_address.get(&address).map(String::as_str)
    }

    pub(super) fn relocated_call_at(&self, address: u32) -> Option<&RelocatedCall> {
        self.relocated_calls_by_address.get(&address)
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
    /// Both successors of a conditional branch are explored. Direct calls are
    /// followed as well as their return continuation, so branch coverage also
    /// includes statically linked children. Indirect calls deliberately stop
    /// that edge: the executor cannot claim coverage for an unknown target.
    pub fn coverage_inventory(&self, symbol: &str) -> Result<CoverageInventory> {
        self.coverage_inventory_with_arguments(symbol, None)
    }

    pub fn coverage_inventory_with_arguments(
        &self,
        symbol: &str,
        arguments: Option<&[u32; 8]>,
    ) -> Result<CoverageInventory> {
        let start = self
            .symbol_address(symbol)
            .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
        let mut pending = vec![start];
        let mut visited = BTreeSet::new();
        let mut inventory = CoverageInventory::default();

        while let Some(address) = pending.pop() {
            if !visited.insert(address) {
                continue;
            }
            if self.symbol_at(address) == Some("ets_delay_us") {
                continue;
            }
            if let Some(call) = self.relocated_call_at(address) {
                if self.relocated_call_link_register(address)? != Reg::ZERO {
                    pending.push(address.wrapping_add(8));
                }
                if let Some(target) = call.target {
                    if self.symbol_at(target) != Some("ets_delay_us") {
                        pending.push(target);
                    }
                } else {
                    inventory
                        .unresolved_edges
                        .insert(address, format!("external-call {}", call.name));
                }
                continue;
            }
            let (instruction, width) = self.instruction(address)?;
            let next = address.wrapping_add(width);
            match instruction {
                Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. } => {
                    inventory.branch_sites.insert(address);
                    pending.push(next);
                    pending.push(address.wrapping_add(offset.as_u32()));
                }
                Inst::Jal { offset, dest } => {
                    pending.push(address.wrapping_add(offset.as_u32()));
                    if dest != Reg::ZERO {
                        pending.push(next);
                    }
                }
                Inst::Jalr { offset, base, dest }
                    if !(dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0) =>
                {
                    // An indirect edge is intentionally not guessed. A normal
                    // call may still return to the following instruction.
                    if dest != Reg::ZERO {
                        pending.push(next);
                    }
                    inventory
                        .unresolved_edges
                        .insert(address, instruction.to_string());
                }
                Inst::Jalr { .. } => {}
                Inst::Ecall | Inst::Ebreak => {}
                _ => pending.push(next),
            }
        }
        let feasible = self.feasible_branch_outcomes(start, arguments)?;
        for site in &inventory.branch_sites {
            for taken in [false, true] {
                if feasible.contains(&(*site, taken)) {
                    inventory.branch_outcomes.insert((*site, taken));
                }
            }
        }
        Ok(inventory)
    }

    pub(super) fn feasible_branch_outcomes(
        &self,
        start: u32,
        arguments: Option<&[u32; 8]>,
    ) -> Result<BTreeSet<(u32, bool)>> {
        const MAX_ABSTRACT_STATES: usize = 200_000;

        let mut initial = [None; 32];
        initial[usize::from(Reg::ZERO.0)] = Some(0);
        if let Some(arguments) = arguments {
            for (index, value) in arguments.iter().copied().enumerate() {
                initial[usize::from(Reg::A0.0) + index] = Some(value);
            }
        }
        let unknown_after_call = || {
            let mut registers = [None; 32];
            registers[usize::from(Reg::ZERO.0)] = Some(0);
            registers
        };
        let mut pending = vec![(start, initial)];
        let mut visited = BTreeSet::new();
        let mut outcomes = BTreeSet::new();

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
                        outcomes.insert((address, outcome));
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
        Ok(outcomes)
    }

    pub(super) fn byte(&self, address: u32) -> Option<u8> {
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
            *byte = self
                .byte(address.wrapping_add(offset as u32))
                .ok_or_else(|| format!("truncated instruction at {address:#x}"))?;
        }
        let (instruction, _) = Inst::decode(u32::from_le_bytes(word), Xlen::Rv32)
            .map_err(|error| format!("cannot decode instruction at {address:#x}: {error}"))?;
        Ok((instruction, width))
    }
}
