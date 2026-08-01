//! Concrete RV32 execution with interceptable MMIO.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::{fs, path::Path};

use object::{
    Object, ObjectSection, ObjectSegment, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionKind, SymbolKind, SymbolSection,
};
use rv_asm::{Inst, Reg, Xlen};

use crate::{Result, SvdMap, binary::andi_immediate};

const RETURN_SENTINEL: u32 = 0xffff_fffc;
const STACK_POINTER: u32 = 0x3fff_f000;
const STACK_SIZE: u32 = 0x1_0000;

fn execution_stack_contains(address: u32) -> bool {
    address
        .checked_sub(STACK_POINTER.wrapping_sub(STACK_SIZE))
        .is_some_and(|offset| offset < STACK_SIZE)
}

#[derive(Clone, Debug)]
struct Segment {
    address: u32,
    bytes: Vec<u8>,
    memory_size: u32,
    writable: bool,
}

#[derive(Clone, Debug)]
struct RelocatedCall {
    name: String,
    target: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ExecutableImage {
    segments: Vec<Segment>,
    symbols_by_name: HashMap<String, u32>,
    symbols_by_address: BTreeMap<u32, String>,
    call_trampoline_addresses: BTreeSet<u32>,
    relocated_calls_by_address: BTreeMap<u32, RelocatedCall>,
    global_pointer: Option<u32>,
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

    fn resolve_external_relocations(&mut self) {
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

    fn symbol_at(&self, address: u32) -> Option<&str> {
        self.symbols_by_address.get(&address).map(String::as_str)
    }

    fn relocated_call_at(&self, address: u32) -> Option<&RelocatedCall> {
        self.relocated_calls_by_address.get(&address)
    }

    pub(crate) fn relocated_calls(&self) -> BTreeMap<u32, (String, Option<u32>)> {
        self.relocated_calls_by_address
            .iter()
            .map(|(address, call)| (*address, (call.name.clone(), call.target)))
            .collect()
    }

    fn relocated_call_link_register(&self, address: u32) -> Result<Reg> {
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

    fn feasible_branch_outcomes(
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

    fn byte(&self, address: u32) -> Option<u8> {
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

    fn contains_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            address
                .checked_sub(segment.address)
                .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    fn contains_writable_memory(&self, address: u32) -> bool {
        self.segments.iter().any(|segment| {
            segment.writable
                && address
                    .checked_sub(segment.address)
                    .is_some_and(|offset| offset < segment.memory_size)
        })
    }

    fn instruction(&self, address: u32) -> Result<(Inst, u32)> {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionEvent {
    Read {
        width: u8,
        address: u32,
        register: String,
        value: u32,
    },
    Write {
        width: u8,
        address: u32,
        register: String,
        value: u32,
    },
    DelayMicros(u32),
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionTimelineEvent {
    Observable(ExecutionEvent),
    Call(OrderedCall),
    Branch { site: u32, taken: bool },
    RamRead { width: u8, address: u32, value: u32 },
    RamWrite { width: u8, address: u32, value: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRange {
    pub start: u32,
    pub length: u32,
}

/// Who is allowed to change a RAM range between two modeled CPU calls.
///
/// `Interrupt`, `Dma`, and `SharedUnknown` are invalidated at every session
/// boundary and must be seeded again by the next scenario before they can be
/// read. `MmioDerived` remains CPU-owned storage whose value was computed from
/// an explicit peripheral response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the ownership schema includes interrupt/DMA domains before a contract needs them"
)]
pub enum MemoryOwner {
    Cpu,
    MmioDerived,
    Interrupt,
    Dma,
    SharedUnknown,
    Immutable,
}

impl MemoryOwner {
    const fn may_change_outside_cpu(self) -> bool {
        matches!(self, Self::Interrupt | Self::Dma | Self::SharedUnknown)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryOwnership {
    pub range: MemoryRange,
    pub owner: MemoryOwner,
}

impl MemoryOwnership {
    fn overlaps(self, other: Self) -> bool {
        let self_end = self.range.start.saturating_add(self.range.length);
        let other_end = other.range.start.saturating_add(other.range.length);
        self.range.start < other_end && other.range.start < self_end
    }
}

/// Memory initialization policy at an [`ExecutionSession`] boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "reset modes are exercised by session clients and validator tests"
)]
pub enum ResetPolicy {
    /// An ordinary function call: retain CPU-owned writable state.
    #[default]
    Continue,
    /// Recreate `.data`/`.bss` from the immutable linked ELF image.
    ColdBoot,
    /// Recreate ELF-backed state but retain explicitly persistent/no-init RAM.
    WarmReset,
}

impl MemoryRange {
    fn contains(self, address: u32) -> bool {
        address
            .checked_sub(self.start)
            .is_some_and(|offset| offset < self.length)
    }
}

/// One observed range whose reported addresses are normalized for comparison
/// with a corresponding range in another ELF image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryAlias {
    pub start: u32,
    pub length: u32,
    pub comparison_start: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryChange {
    pub address: u32,
    pub before: u8,
    pub after: u8,
}

struct MmioValue;

impl MmioValue {
    const fn mask(width: u8) -> u32 {
        match width {
            8 => 0xff,
            16 => 0xffff,
            _ => u32::MAX,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndirectCall {
    pub site: u32,
    pub symbol: String,
    pub arguments: [u32; 8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedCall {
    pub site: u32,
    pub symbol: String,
    pub arguments: [u32; 8],
}

#[derive(Clone, Debug, Default)]
pub struct Scenario {
    pub arguments: Vec<u32>,
    pub mmio_initial: BTreeMap<u32, u32>,
    pub mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    pub memory_initial: BTreeMap<u32, u8>,
    pub observed_memory: Vec<MemoryRange>,
    pub memory_aliases: Vec<MemoryAlias>,
    /// Non-ELF RAM that must survive between calls made through an
    /// [`ExecutionSession`]. ELF-backed `.data`/`.bss` is retained
    /// automatically; the private executor stack is fresh for every call.
    pub persistent_memory: Vec<MemoryRange>,
    /// Reviewed ownership of RAM that can outlive this call. Externally owned
    /// ranges become poison at every call boundary unless explicitly seeded.
    pub memory_ownership: Vec<MemoryOwnership>,
    pub reset_policy: ResetPolicy,
    pub max_steps: u64,
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub events: Vec<ExecutionEvent>,
    pub timeline: Vec<ExecutionTimelineEvent>,
    pub return_value: u32,
    pub steps: u64,
    pub branches: BTreeSet<(u32, bool)>,
    pub ordered_branches: Vec<(u32, bool)>,
    pub calls: BTreeSet<String>,
    pub ordered_calls: Vec<OrderedCall>,
    pub indirect_calls: BTreeSet<IndirectCall>,
    pub memory_changes: Vec<MemoryChange>,
    /// Explicit RAM overlay at function entry. Semantic normalizers combine
    /// this with ordered writes when they need a call-time value rather than
    /// the immutable ELF baseline or the final persistent state.
    pub initial_memory: BTreeMap<u32, u8>,
    /// Final bytes eligible for reuse by [`ExecutionSession`]. This contains
    /// ELF-backed writes and explicitly declared persistent RAM, never the
    /// executor's private stack.
    pub persistent_memory: BTreeMap<u32, u8>,
}

/// Persistent software memory across a sequence of vendor calls.
///
/// The linked ELF remains the immutable load baseline. Only writes to its
/// load segments and ranges explicitly declared through
/// [`Scenario::persistent_memory`] are carried into the next invocation.
/// MMIO responses and the private call stack are deliberately per-scenario.
#[derive(Clone, Debug, Default)]
pub struct ExecutionSession {
    memory: BTreeMap<u32, u8>,
    persistent_ranges: Vec<MemoryRange>,
    memory_ownership: Vec<MemoryOwnership>,
}

impl ExecutionSession {
    pub fn execute(
        &mut self,
        image: &ExecutableImage,
        svd: &SvdMap,
        symbol: &str,
        mut scenario: Scenario,
    ) -> Result<ExecutionResult> {
        for ownership in scenario.memory_ownership.drain(..) {
            if let Some(previous) = self
                .memory_ownership
                .iter()
                .find(|previous| previous.overlaps(ownership) && previous.owner != ownership.owner)
            {
                return Err(format!(
                    "conflicting RAM ownership {:?} and {:?} for overlapping ranges at {:#010x}",
                    previous.owner, ownership.owner, ownership.range.start
                )
                .into());
            }
            if !self.memory_ownership.contains(&ownership) {
                self.memory_ownership.push(ownership);
            }
        }
        for range in scenario.persistent_memory.drain(..) {
            if !self.persistent_ranges.contains(&range) {
                self.persistent_ranges.push(range);
            }
        }
        match scenario.reset_policy {
            ResetPolicy::Continue => {}
            ResetPolicy::ColdBoot => self.memory.clear(),
            ResetPolicy::WarmReset => self.memory.retain(|address, _| {
                self.persistent_ranges
                    .iter()
                    .any(|range| range.contains(*address))
            }),
        }
        for ownership in &self.memory_ownership {
            if ownership.owner.may_change_outside_cpu() {
                self.memory
                    .retain(|address, _| !ownership.range.contains(*address));
            }
        }
        scenario.persistent_memory = self.persistent_ranges.clone();
        scenario.memory_ownership = self.memory_ownership.clone();

        let explicit = std::mem::take(&mut scenario.memory_initial);
        scenario.memory_initial = self.memory.clone();
        scenario.memory_initial.extend(explicit);

        let result = execute(image, svd, symbol, scenario)?;
        self.memory.clone_from(&result.persistent_memory);
        Ok(result)
    }

    pub fn byte(&self, image: &ExecutableImage, address: u32) -> Option<u8> {
        if self.memory_ownership.iter().any(|ownership| {
            ownership.range.contains(address) && ownership.owner.may_change_outside_cpu()
        }) {
            return None;
        }
        self.memory
            .get(&address)
            .copied()
            .or_else(|| image.loaded_byte(address))
    }
}

struct Machine<'a> {
    image: &'a ExecutableImage,
    svd: &'a SvdMap,
    registers: [u32; 32],
    pc: u32,
    overlay: BTreeMap<u32, u8>,
    initial_overlay: BTreeMap<u32, u8>,
    observed_memory: Vec<MemoryRange>,
    memory_aliases: Vec<MemoryAlias>,
    persistent_memory: Vec<MemoryRange>,
    memory_ownership: Vec<MemoryOwnership>,
    /// Explicit stable read values supplied by the scenario. Bus writes do
    /// not update this map: storage/W1C/FIFO semantics belong to an explicit
    /// peripheral model, not to the generic transaction recorder.
    mmio_read_seeds: BTreeMap<u32, u32>,
    mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    events: Vec<ExecutionEvent>,
    timeline: Vec<ExecutionTimelineEvent>,
    branches: BTreeSet<(u32, bool)>,
    ordered_branches: Vec<(u32, bool)>,
    calls: BTreeSet<String>,
    ordered_calls: Vec<OrderedCall>,
    indirect_calls: BTreeSet<IndirectCall>,
    steps: u64,
    max_steps: u64,
}

impl<'a> Machine<'a> {
    fn new(image: &'a ExecutableImage, svd: &'a SvdMap, start: u32, scenario: Scenario) -> Self {
        let mut registers = [0_u32; 32];
        registers[usize::from(Reg::RA.0)] = RETURN_SENTINEL;
        registers[usize::from(Reg::SP.0)] = STACK_POINTER;
        if let Some(global_pointer) = image.global_pointer {
            registers[usize::from(Reg::GP.0)] = global_pointer;
        }
        for (index, value) in scenario.arguments.into_iter().take(8).enumerate() {
            registers[10 + index] = value;
        }
        let initial_overlay = scenario.memory_initial;
        Self {
            image,
            svd,
            registers,
            pc: start,
            overlay: initial_overlay.clone(),
            initial_overlay,
            observed_memory: scenario.observed_memory,
            memory_aliases: scenario.memory_aliases,
            persistent_memory: scenario.persistent_memory,
            memory_ownership: scenario.memory_ownership,
            mmio_read_seeds: scenario.mmio_initial,
            mmio_reads: scenario.mmio_reads,
            events: Vec::new(),
            timeline: Vec::new(),
            branches: BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            steps: 0,
            max_steps: if scenario.max_steps == 0 {
                100_000
            } else {
                scenario.max_steps
            },
        }
    }

    fn register(&self, register: Reg) -> u32 {
        self.registers[usize::from(register.0)]
    }

    fn set_register(&mut self, register: Reg, value: u32) {
        if register != Reg::ZERO {
            self.registers[usize::from(register.0)] = value;
        }
    }

    fn normal_byte(&self, address: u32) -> Result<u8> {
        if let Some(value) = self.overlay.get(&address).copied() {
            return Ok(value);
        }
        if self.memory_ownership.iter().any(|ownership| {
            ownership.range.contains(address) && ownership.owner.may_change_outside_cpu()
        }) {
            return Err(format!(
                "read from externally mutable RAM at {address:#010x} without a call-entry seed"
            )
            .into());
        }
        self.image
            .byte(address)
            .ok_or_else(|| format!("read from poison/unmapped memory at {address:#010x}").into())
    }

    fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        if self.svd.contains_mmio(address) {
            let sequenced = self
                .mmio_reads
                .get_mut(&address)
                .and_then(VecDeque::pop_front);
            let value = match sequenced {
                Some(value) => value & MmioValue::mask(width),
                None => self
                    .mmio_read_seeds
                    .get(&address)
                    .copied()
                    .map(|value| value & MmioValue::mask(width))
                    .ok_or_else(|| {
                        format!("MMIO read at {address:#010x} has no explicit seed or response")
                    })?,
            };
            self.record_event(ExecutionEvent::Read {
                width,
                address,
                register: self.svd.register_name(address),
                value,
            });
            return Ok(value);
        }
        let bytes = usize::from(width / 8);
        let mut value = 0_u32;
        for offset in 0..bytes {
            value |=
                u32::from(self.normal_byte(address.wrapping_add(offset as u32))?) << (offset * 8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamRead {
            width,
            address,
            value,
        });
        Ok(value)
    }

    fn normal_address_is_valid(&self, address: u32) -> bool {
        self.image.contains_memory(address)
            || execution_stack_contains(address)
            || self.initial_overlay.contains_key(&address)
            || self
                .observed_memory
                .iter()
                .any(|range| range.contains(address))
            || self.memory_aliases.iter().any(|alias| {
                address
                    .checked_sub(alias.start)
                    .is_some_and(|offset| offset < alias.length)
            })
    }

    fn write(&mut self, address: u32, width: u8, value: u32) -> Result<()> {
        let bytes = usize::from(width / 8);
        if self.svd.contains_mmio(address) {
            self.record_event(ExecutionEvent::Write {
                width,
                address,
                register: self.svd.register_name(address),
                value: value & MmioValue::mask(width),
            });
            return Ok(());
        }
        for offset in 0..bytes {
            let byte_address = address.wrapping_add(offset as u32);
            if self.memory_ownership.iter().any(|ownership| {
                ownership.range.contains(byte_address) && ownership.owner == MemoryOwner::Immutable
            }) {
                return Err(format!(
                    "write to ownership-declared immutable RAM at {byte_address:#010x}"
                )
                .into());
            }
            if self.image.contains_memory(byte_address)
                && !self.image.contains_writable_memory(byte_address)
            {
                return Err(
                    format!("write to read-only ELF memory at {byte_address:#010x}").into(),
                );
            }
            if !self.normal_address_is_valid(byte_address) {
                return Err(format!("write to undeclared memory at {byte_address:#010x}").into());
            }
            self.overlay
                .insert(byte_address, (value >> (offset * 8)) as u8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamWrite {
            width,
            address,
            value: value & MmioValue::mask(width),
        });
        Ok(())
    }

    fn branch(&mut self, taken: bool, offset: i32, width: u32) {
        self.branches.insert((self.pc, taken));
        self.ordered_branches.push((self.pc, taken));
        self.timeline.push(ExecutionTimelineEvent::Branch {
            site: self.pc,
            taken,
        });
        self.pc = if taken {
            self.pc.wrapping_add(offset as u32)
        } else {
            self.pc.wrapping_add(width)
        };
    }

    fn record_call(&mut self, site: u32, symbol: String) {
        let arguments =
            core::array::from_fn(|index| self.registers[usize::from(Reg::A0.0) + index]);
        self.calls.insert(symbol.clone());
        let call = OrderedCall {
            site,
            symbol,
            arguments,
        };
        self.ordered_calls.push(call.clone());
        self.timeline.push(ExecutionTimelineEvent::Call(call));
    }

    fn record_event(&mut self, event: ExecutionEvent) {
        self.events.push(event.clone());
        self.timeline
            .push(ExecutionTimelineEvent::Observable(event));
    }

    fn memory_changes(&self) -> Result<Vec<MemoryChange>> {
        let mut observed_addresses: BTreeMap<u32, u32> = self
            .observed_memory
            .iter()
            .flat_map(|range| {
                (0..range.length).map(move |offset| {
                    let address = range.start.wrapping_add(offset);
                    (address, address)
                })
            })
            .collect();
        for alias in &self.memory_aliases {
            for offset in 0..alias.length {
                observed_addresses.insert(
                    alias.comparison_start.wrapping_add(offset),
                    alias.start.wrapping_add(offset),
                );
            }
        }
        let mut changes = Vec::new();
        for (comparison_address, address) in observed_addresses {
            let before = self
                .initial_overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| {
                    format!("observed memory at {address:#010x} has no explicit initial value")
                })?;
            let after = self
                .overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| format!("observed memory at {address:#010x} remains poison"))?;
            if before != after {
                changes.push(MemoryChange {
                    address: comparison_address,
                    before,
                    after,
                });
            }
        }
        Ok(changes)
    }

    fn persistent_memory(&self) -> BTreeMap<u32, u8> {
        self.overlay
            .iter()
            .filter_map(|(address, value)| {
                let explicitly_persistent = self
                    .persistent_memory
                    .iter()
                    .any(|range| range.contains(*address));
                ((!execution_stack_contains(*address)
                    && self.image.contains_writable_memory(*address))
                    || explicitly_persistent)
                    .then_some((*address, *value))
            })
            .collect()
    }

    fn step(&mut self) -> Result<bool> {
        if self.steps == self.max_steps {
            let context = self
                .ordered_calls
                .iter()
                .rev()
                .take(6)
                .map(|call| {
                    format!(
                        "{}({:#x},{:#x},{:#x},{:#x})",
                        call.symbol,
                        call.arguments[0],
                        call.arguments[1],
                        call.arguments[2],
                        call.arguments[3]
                    )
                })
                .collect::<Vec<_>>()
                .join(" <- ");
            return Err(format!(
                "execution exceeded {} steps at pc={:#010x}; recent calls: {}",
                self.max_steps,
                self.pc,
                if context.is_empty() {
                    "<entry>"
                } else {
                    &context
                }
            )
            .into());
        }
        self.steps += 1;

        if let Some(symbol) = self.image.symbol_at(self.pc)
            && symbol == "ets_delay_us"
        {
            self.record_event(ExecutionEvent::DelayMicros(self.register(Reg::A0)));
            let return_address = self.register(Reg::RA);
            if return_address == RETURN_SENTINEL {
                return Ok(false);
            }
            self.pc = return_address;
            return Ok(true);
        }
        if let Some(call) = self.image.relocated_call_at(self.pc).cloned() {
            let link = self.image.relocated_call_link_register(self.pc)?;
            let continuation = self.pc.wrapping_add(8);
            if link != Reg::ZERO {
                self.set_register(link, continuation);
            }
            if let Some(target) = call.target {
                self.record_call(self.pc, call.name);
                self.pc = target;
            } else {
                return Err(format!(
                    "execution reached unresolved external call {} at {:#010x}",
                    call.name, self.pc
                )
                .into());
            }
            return Ok(true);
        }

        let (instruction, width) = self.image.instruction(self.pc)?;
        let next = self.pc.wrapping_add(width);
        match instruction {
            Inst::Lui { uimm, dest } => self.set_register(dest, uimm.as_u32()),
            Inst::Auipc { uimm, dest } => {
                self.set_register(dest, self.pc.wrapping_add(uimm.as_u32()));
            }
            Inst::Addi { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1).wrapping_add(imm.as_u32()));
            }
            Inst::Andi { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) & andi_immediate(imm, width as u8));
            }
            Inst::Ori { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) | imm.as_u32());
            }
            Inst::Xori { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) ^ imm.as_u32());
            }
            Inst::Slli { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) << (imm.as_u32() & 31));
            }
            Inst::Srli { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) >> (imm.as_u32() & 31));
            }
            Inst::Srai { imm, dest, src1 } => self.set_register(
                dest,
                ((self.register(src1) as i32) >> (imm.as_u32() & 31)) as u32,
            ),
            Inst::Slti { imm, dest, src1 } => {
                self.set_register(dest, u32::from((self.register(src1) as i32) < imm.as_i32()));
            }
            Inst::Sltiu { imm, dest, src1 } => {
                self.set_register(dest, u32::from(self.register(src1) < imm.as_u32()));
            }
            Inst::Add { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_add(self.register(src2)))
            }
            Inst::Sub { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_sub(self.register(src2)))
            }
            Inst::And { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) & self.register(src2));
            }
            Inst::Or { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) | self.register(src2));
            }
            Inst::Xor { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) ^ self.register(src2));
            }
            Inst::Sll { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) << (self.register(src2) & 31));
            }
            Inst::Srl { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) >> (self.register(src2) & 31));
            }
            Inst::Sra { dest, src1, src2 } => self.set_register(
                dest,
                ((self.register(src1) as i32) >> (self.register(src2) & 31)) as u32,
            ),
            Inst::Slt { dest, src1, src2 } => self.set_register(
                dest,
                u32::from((self.register(src1) as i32) < (self.register(src2) as i32)),
            ),
            Inst::Sltu { dest, src1, src2 } => {
                self.set_register(dest, u32::from(self.register(src1) < self.register(src2)));
            }
            Inst::Mul { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_mul(self.register(src2)))
            }
            Inst::Mulh { dest, src1, src2 } => self.set_register(
                dest,
                (((self.register(src1) as i32 as i64) * (self.register(src2) as i32 as i64)) >> 32)
                    as u32,
            ),
            Inst::Mulhsu { dest, src1, src2 } => self.set_register(
                dest,
                (((self.register(src1) as i32 as i64) * i64::from(self.register(src2))) >> 32)
                    as u32,
            ),
            Inst::Mulhu { dest, src1, src2 } => self.set_register(
                dest,
                ((u64::from(self.register(src1)) * u64::from(self.register(src2))) >> 32) as u32,
            ),
            Inst::Div { dest, src1, src2 } => {
                let left = self.register(src1) as i32;
                let right = self.register(src2) as i32;
                self.set_register(
                    dest,
                    if right == 0 {
                        u32::MAX
                    } else if left == i32::MIN && right == -1 {
                        i32::MIN as u32
                    } else {
                        (left / right) as u32
                    },
                );
            }
            Inst::Divu { dest, src1, src2 } => {
                let right = self.register(src2);
                self.set_register(
                    dest,
                    self.register(src1).checked_div(right).unwrap_or(u32::MAX),
                );
            }
            Inst::Rem { dest, src1, src2 } => {
                let left = self.register(src1) as i32;
                let right = self.register(src2) as i32;
                self.set_register(
                    dest,
                    if right == 0 {
                        left as u32
                    } else if left == i32::MIN && right == -1 {
                        0
                    } else {
                        (left % right) as u32
                    },
                );
            }
            Inst::Remu { dest, src1, src2 } => {
                let right = self.register(src2);
                self.set_register(
                    dest,
                    if right == 0 {
                        self.register(src1)
                    } else {
                        self.register(src1) % right
                    },
                );
            }
            Inst::Lb { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = (self.read(address, 8)? as u8 as i8 as i32) as u32;
                self.set_register(dest, value);
            }
            Inst::Lbu { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 8)? & 0xff;
                self.set_register(dest, value);
            }
            Inst::Lh { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = (self.read(address, 16)? as u16 as i16 as i32) as u32;
                self.set_register(dest, value);
            }
            Inst::Lhu { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 16)? & 0xffff;
                self.set_register(dest, value);
            }
            Inst::Lw { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 32)?;
                self.set_register(dest, value);
            }
            Inst::Sb { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    8,
                    self.register(src),
                )?;
            }
            Inst::Sh { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    16,
                    self.register(src),
                )?;
            }
            Inst::Sw { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    32,
                    self.register(src),
                )?;
            }
            Inst::Beq { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) == self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bne { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) != self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Blt { offset, src1, src2 } => {
                self.branch(
                    (self.register(src1) as i32) < (self.register(src2) as i32),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bge { offset, src1, src2 } => {
                self.branch(
                    (self.register(src1) as i32) >= (self.register(src2) as i32),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bltu { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) < self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bgeu { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) >= self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Jal { offset, dest } => {
                let target = self.pc.wrapping_add(offset.as_u32());
                self.set_register(dest, next);
                let leaves_call_trampoline =
                    self.image.call_trampoline_addresses.contains(&self.pc);
                if !leaves_call_trampoline && let Some(symbol) = self.image.symbol_at(target) {
                    self.record_call(self.pc, symbol.to_owned());
                }
                self.pc = target;
                return Ok(true);
            }
            Inst::Jalr { offset, base, dest } => {
                let target = self.register(base).wrapping_add(offset.as_u32()) & !1;
                self.set_register(dest, next);
                if target == RETURN_SENTINEL {
                    return Ok(false);
                }
                if let Some(symbol) = self.image.symbol_at(target) {
                    let is_return = dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0;
                    if !is_return {
                        self.record_call(self.pc, symbol.to_owned());
                        self.indirect_calls.insert(IndirectCall {
                            site: self.pc,
                            symbol: symbol.to_owned(),
                            arguments: core::array::from_fn(|index| {
                                self.registers[usize::from(Reg::A0.0) + index]
                            }),
                        });
                    }
                }
                self.pc = target;
                return Ok(true);
            }
            Inst::Fence { fence } => {
                let encode = |set: rv_asm::FenceSet| {
                    u8::from(set.device_input) << 3
                        | u8::from(set.device_output) << 2
                        | u8::from(set.memory_read) << 1
                        | u8::from(set.memory_write)
                };
                self.record_event(ExecutionEvent::Fence {
                    fm: fence.fm,
                    predecessor: encode(fence.pred),
                    successor: encode(fence.succ),
                });
            }
            _ => {
                return Err(
                    format!("unsupported instruction at {:#x}: {instruction}", self.pc).into(),
                );
            }
        }
        self.pc = next;
        self.registers[0] = 0;
        Ok(true)
    }
}

pub fn execute(
    image: &ExecutableImage,
    svd: &SvdMap,
    symbol: &str,
    scenario: Scenario,
) -> Result<ExecutionResult> {
    if scenario.arguments.len() > 8 {
        return Err(format!(
            "{} arguments were provided, but stack arguments are not implemented; maximum is 8",
            scenario.arguments.len()
        )
        .into());
    }
    let start = image
        .symbol_address(symbol)
        .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
    let mut machine = Machine::new(image, svd, start, scenario);
    while machine.step()? {}
    let unconsumed: Vec<_> = machine
        .mmio_reads
        .iter()
        .filter_map(|(address, values)| (!values.is_empty()).then_some((*address, values.len())))
        .collect();
    if !unconsumed.is_empty() {
        return Err(format!("unconsumed MMIO read responses: {unconsumed:?}").into());
    }
    let return_value = machine.register(Reg::A0);
    let memory_changes = machine.memory_changes()?;
    let persistent_memory = machine.persistent_memory();
    let initial_memory = machine.initial_overlay.clone();
    Ok(ExecutionResult {
        events: machine.events,
        timeline: machine.timeline,
        return_value,
        steps: machine.steps,
        branches: machine.branches,
        ordered_branches: machine.ordered_branches,
        calls: machine.calls,
        ordered_calls: machine.ordered_calls,
        indirect_calls: machine.indirect_calls,
        memory_changes,
        initial_memory,
        persistent_memory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_image(bytes: Vec<u8>, memory_size: u32) -> ExecutableImage {
        ExecutableImage {
            segments: vec![Segment {
                address: 0x1000,
                bytes,
                memory_size,
                writable: true,
            }],
            symbols_by_name: HashMap::from([("test".to_owned(), 0x1000)]),
            symbols_by_address: BTreeMap::from([(0x1000, "test".to_owned())]),
            call_trampoline_addresses: BTreeSet::new(),
            relocated_calls_by_address: BTreeMap::new(),
            global_pointer: None,
        }
    }

    fn empty_svd() -> SvdMap {
        SvdMap {
            registers: Vec::new(),
            windows: Vec::new(),
        }
    }

    fn tail_relocation_image(target: Option<u32>) -> ExecutableImage {
        let mut symbols_by_name = HashMap::from([("wrapper".to_owned(), 0x1000)]);
        let mut symbols_by_address = BTreeMap::from([(0x1000, "wrapper".to_owned())]);
        let mut segments = vec![Segment {
            address: 0x1000,
            bytes: vec![
                0x17, 0x03, 0x00, 0x00, // auipc t1, 0
                0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
                0x63, 0x00, 0x00, 0x00, // beq zero, zero, 0 (must be unreachable)
            ],
            memory_size: 12,
            writable: true,
        }];
        if let Some(target) = target {
            symbols_by_name.insert("callee".to_owned(), target);
            symbols_by_address.insert(target, "callee".to_owned());
            segments.push(Segment {
                address: target,
                bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
                memory_size: 4,
                writable: true,
            });
        }
        ExecutableImage {
            segments,
            symbols_by_name,
            symbols_by_address,
            call_trampoline_addresses: BTreeSet::new(),
            relocated_calls_by_address: BTreeMap::from([(
                0x1000,
                RelocatedCall {
                    name: "callee".to_owned(),
                    target: None,
                },
            )]),
            global_pointer: None,
        }
    }

    fn oracle() -> Option<std::path::PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("_oracles/esp32s31_rev0_rom.elf");
        path.exists().then_some(path)
    }

    #[test]
    fn executes_frequency_band_tail_call_and_records_both_mmio_updates() {
        let Some(oracle) = oracle() else {
            eprintln!("private ROM fixture is not installed; integration test skipped");
            return;
        };
        let image = ExecutableImage::load(&oracle).unwrap();
        let svd = SvdMap::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("svd/esp32s31-radio.svd"),
        )
        .unwrap();
        let mut scenario = Scenario {
            arguments: vec![1],
            ..Scenario::default()
        };
        scenario.mmio_initial.insert(0x2010_7030, u32::MAX);
        scenario.mmio_initial.insert(0x2010_7ce4, 0);
        let result = execute(&image, &svd, "phy_freq_band_reg_set", scenario).unwrap();
        assert_eq!(result.events.len(), 4);
        assert_eq!(
            result.events[1],
            ExecutionEvent::Write {
                width: 32,
                address: 0x2010_7030,
                register: "PHY_AGC_ORACLE.AGC_ANTENNA_CONTROL".to_owned(),
                value: !(1 << 5),
            }
        );
        assert_eq!(
            result.events[3],
            ExecutionEvent::Write {
                width: 32,
                address: 0x2010_7ce4,
                register: "PHY_FREQUENCY_CHANNEL_ORACLE.CHANNEL_CBW_CONTROL_1".to_owned(),
                value: 1 << 5,
            }
        );
        assert!(result.calls.contains("phy_vht_support"));
    }

    #[test]
    fn top_level_tail_delay_finishes_at_the_return_sentinel() {
        let Some(oracle) = oracle() else {
            eprintln!("private ROM fixture is not installed; integration test skipped");
            return;
        };
        let image = ExecutableImage::load(&oracle).unwrap();
        let svd = SvdMap::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("svd/esp32s31-radio.svd"),
        )
        .unwrap();
        let mut scenario = Scenario::default();
        scenario.mmio_initial.insert(0x2010_001c, 0);
        let result = execute(&image, &svd, "phy_dis_hw_set_freq", scenario).unwrap();
        assert!(matches!(
            result.events.last(),
            Some(ExecutionEvent::DelayMicros(2))
        ));
    }

    #[test]
    fn static_branch_inventory_includes_reachable_child_control_flow() {
        let Some(oracle) = oracle() else {
            eprintln!("private ROM fixture is not installed; integration test skipped");
            return;
        };
        let image = ExecutableImage::load(&oracle).unwrap();
        assert!(
            !image
                .coverage_inventory("phy_bb_bss_cbw40")
                .unwrap()
                .branch_sites
                .is_empty()
        );
    }

    #[test]
    fn branch_inventory_removes_child_outcomes_infeasible_from_fixed_arguments() {
        let Some(oracle) = oracle() else {
            eprintln!("private ROM fixture is not installed; integration test skipped");
            return;
        };
        let image = ExecutableImage::load(&oracle).unwrap();
        let wrapper = image.coverage_inventory("phy_pbus_debugmode").unwrap();
        assert_eq!(wrapper.branch_outcomes.len(), 1);
        assert!(wrapper.branch_outcomes.iter().all(|(_, taken)| !taken));

        let child = image.coverage_inventory("phy_pbus_force_mode").unwrap();
        assert!(child.branch_outcomes.iter().any(|(_, taken)| *taken));
        assert!(child.branch_outcomes.iter().any(|(_, taken)| !*taken));
    }

    #[test]
    fn companion_symbol_resolves_external_tail_relocation_without_fallthrough() {
        let mut image = tail_relocation_image(Some(0x2000));
        image.resolve_external_relocations();
        assert_eq!(
            image.relocated_call_at(0x1000).and_then(|call| call.target),
            Some(0x2000)
        );
        let inventory = image.coverage_inventory("wrapper").unwrap();
        assert!(inventory.unresolved_edges.is_empty());
        assert!(inventory.branch_sites.is_empty());

        let svd = SvdMap {
            registers: Vec::new(),
            windows: vec![crate::Window { start: 0, end: 1 }],
        };
        let result = execute(&image, &svd, "wrapper", Scenario::default()).unwrap();
        assert!(result.calls.contains("callee"));
        assert_eq!(result.ordered_calls.len(), 1);
        assert_eq!(result.ordered_calls[0].symbol, "callee");
        assert!(result.events.is_empty());
    }

    #[test]
    fn call_trampoline_does_not_duplicate_the_ordered_target_call() {
        let image = ExecutableImage {
            segments: vec![
                Segment {
                    address: 0x1000,
                    bytes: vec![
                        0x97, 0x02, 0x00, 0x00, // auipc t0, 0
                        0x67, 0x80, 0x02, 0x00, // jalr zero, 0(t0)
                    ],
                    memory_size: 8,
                    writable: true,
                },
                Segment {
                    address: 0x2000,
                    bytes: [0x6f, 0x00, 0x00, 0x01]
                        .into_iter()
                        .chain([0; 12])
                        .chain([0x67, 0x80, 0x00, 0x00])
                        .collect(),
                    memory_size: 20,
                    writable: true,
                },
            ],
            symbols_by_name: HashMap::from([
                ("wrapper".to_owned(), 0x1000),
                ("__call_callee".to_owned(), 0x2000),
                ("callee".to_owned(), 0x2010),
            ]),
            symbols_by_address: BTreeMap::from([
                (0x1000, "wrapper".to_owned()),
                (0x2000, "__call_callee".to_owned()),
                (0x2010, "callee".to_owned()),
            ]),
            call_trampoline_addresses: BTreeSet::from([0x2000]),
            relocated_calls_by_address: BTreeMap::from([(
                0x1000,
                RelocatedCall {
                    name: "callee".to_owned(),
                    target: Some(0x2000),
                },
            )]),
            global_pointer: None,
        };
        let result = execute(&image, &empty_svd(), "wrapper", Scenario::default()).unwrap();
        assert_eq!(result.ordered_calls.len(), 1);
        assert_eq!(result.ordered_calls[0].symbol, "callee");
    }

    #[test]
    fn ordered_control_flow_retains_call_multiplicity_and_loop_iterations() {
        let calls = ExecutableImage {
            segments: vec![Segment {
                address: 0x1000,
                bytes: vec![
                    0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
                    0xef, 0x00, 0x00, 0x01, // jal ra, 16
                    0xef, 0x00, 0xc0, 0x00, // jal ra, 12
                    0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
                    0x67, 0x80, 0x00, 0x00, // ret
                    0x67, 0x80, 0x00, 0x00, // callee: ret
                ],
                memory_size: 24,
                writable: true,
            }],
            symbols_by_name: HashMap::from([
                ("wrapper".to_owned(), 0x1000),
                ("callee".to_owned(), 0x1014),
            ]),
            symbols_by_address: BTreeMap::from([
                (0x1000, "wrapper".to_owned()),
                (0x1014, "callee".to_owned()),
            ]),
            call_trampoline_addresses: BTreeSet::new(),
            relocated_calls_by_address: BTreeMap::new(),
            global_pointer: None,
        };
        let result = execute(&calls, &empty_svd(), "wrapper", Scenario::default()).unwrap();
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.ordered_calls.len(), 2);
        assert!(
            result
                .ordered_calls
                .iter()
                .all(|call| call.symbol == "callee")
        );

        let loop_image = tiny_image(
            vec![
                0x13, 0x05, 0x30, 0x00, // addi a0, zero, 3
                0x13, 0x05, 0xf5, 0xff, // addi a0, a0, -1
                0xe3, 0x1e, 0x05, 0xfe, // bne a0, zero, -4
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            16,
        );
        let result = execute(&loop_image, &empty_svd(), "test", Scenario::default()).unwrap();
        assert_eq!(result.branches.len(), 2);
        assert_eq!(
            result.ordered_branches,
            vec![(0x1008, true), (0x1008, true), (0x1008, false)]
        );
    }

    #[test]
    fn ordered_timeline_retains_intermediate_ram_values() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let address = 0x3fff_0000;
        let mut scenario = Scenario::default();
        scenario
            .memory_initial
            .extend((0..4).map(|offset| (address + offset, 0)));
        let svd = empty_svd();
        let mut machine = Machine::new(&image, &svd, 0x1000, scenario);

        machine.write(address, 32, 0x1122_3344).unwrap();
        assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);
        machine.write(address, 32, 0x5566_7788).unwrap();
        assert_eq!(machine.read(address, 32).unwrap(), 0x5566_7788);

        assert_eq!(
            machine.timeline,
            vec![
                ExecutionTimelineEvent::RamWrite {
                    width: 32,
                    address,
                    value: 0x1122_3344,
                },
                ExecutionTimelineEvent::RamRead {
                    width: 32,
                    address,
                    value: 0x1122_3344,
                },
                ExecutionTimelineEvent::RamWrite {
                    width: 32,
                    address,
                    value: 0x5566_7788,
                },
                ExecutionTimelineEvent::RamRead {
                    width: 32,
                    address,
                    value: 0x5566_7788,
                },
            ]
        );
    }

    #[test]
    fn execution_session_retains_elf_and_declared_ram_but_not_stack() {
        let mut image = tiny_image(
            vec![
                0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
                0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
                0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0, 0, 0, 0, // ELF-backed mutable word
                0x13, 0x01, 0xc1, 0xff, // stack_writer: addi sp, sp, -4
                0x23, 0x20, 0xa1, 0x00, // stack_writer: sw a0, 0(sp)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            32,
        );
        image
            .symbols_by_name
            .insert("stack_writer".to_owned(), 0x1014);
        image
            .symbols_by_address
            .insert(0x1014, "stack_writer".to_owned());
        let svd = empty_svd();
        let mut session = ExecutionSession::default();

        for expected in [1, 2] {
            let scenario = Scenario {
                arguments: vec![0, 0x1010],
                ..Scenario::default()
            };
            let result = session.execute(&image, &svd, "test", scenario).unwrap();
            assert_eq!(result.return_value, expected);
        }
        assert_eq!(session.byte(&image, 0x1010), Some(2));

        let external = 0x2000;
        let first = Scenario {
            arguments: vec![0, external],
            memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
            persistent_memory: vec![MemoryRange {
                start: external,
                length: 4,
            }],
            ..Scenario::default()
        };
        assert_eq!(
            session
                .execute(&image, &svd, "test", first)
                .unwrap()
                .return_value,
            1
        );
        let second = Scenario {
            arguments: vec![0, external],
            ..Scenario::default()
        };
        assert_eq!(
            session
                .execute(&image, &svd, "test", second)
                .unwrap()
                .return_value,
            2
        );

        let stack = session
            .execute(
                &image,
                &svd,
                "stack_writer",
                Scenario {
                    arguments: vec![0xdead_beef],
                    ..Scenario::default()
                },
            )
            .unwrap();
        assert!(
            stack
                .persistent_memory
                .keys()
                .all(|address| !execution_stack_contains(*address))
        );
    }

    #[test]
    fn execution_session_invalidates_externally_mutable_ram_between_calls() {
        let image = tiny_image(
            vec![
                0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
                0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
                0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0, 0, 0, 0,
            ],
            20,
        );
        let range = MemoryRange {
            start: 0x1010,
            length: 4,
        };
        let ownership = MemoryOwnership {
            range,
            owner: MemoryOwner::SharedUnknown,
        };
        let mut session = ExecutionSession::default();
        let unseeded = Scenario {
            arguments: vec![0, range.start],
            memory_ownership: vec![ownership],
            ..Scenario::default()
        };
        let error = session
            .execute(&image, &empty_svd(), "test", unseeded)
            .unwrap_err();
        assert!(error.to_string().contains("externally mutable RAM"));

        let seeded = Scenario {
            arguments: vec![0, range.start],
            memory_initial: (0..4)
                .map(|offset| (range.start + offset, u8::from(offset == 0) * 9))
                .collect(),
            ..Scenario::default()
        };
        assert_eq!(
            session
                .execute(&image, &empty_svd(), "test", seeded)
                .unwrap()
                .return_value,
            10
        );
        assert_eq!(session.byte(&image, range.start), None);
    }

    #[test]
    fn execution_session_distinguishes_cold_and_warm_reset() {
        let image = tiny_image(
            vec![
                0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
                0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1
                0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0, 0, 0, 0,
            ],
            20,
        );
        let mut session = ExecutionSession::default();
        for (reset_policy, expected) in [
            (ResetPolicy::Continue, 1),
            (ResetPolicy::Continue, 2),
            (ResetPolicy::ColdBoot, 1),
        ] {
            let result = session
                .execute(
                    &image,
                    &empty_svd(),
                    "test",
                    Scenario {
                        arguments: vec![0, 0x1010],
                        reset_policy,
                        ..Scenario::default()
                    },
                )
                .unwrap();
            assert_eq!(result.return_value, expected);
        }

        let external = 0x2000;
        let first = Scenario {
            arguments: vec![0, external],
            memory_initial: (0..4).map(|offset| (external + offset, 0)).collect(),
            persistent_memory: vec![MemoryRange {
                start: external,
                length: 4,
            }],
            ..Scenario::default()
        };
        assert_eq!(
            session
                .execute(&image, &empty_svd(), "test", first)
                .unwrap()
                .return_value,
            1
        );
        let warm = Scenario {
            arguments: vec![0, external],
            reset_policy: ResetPolicy::WarmReset,
            ..Scenario::default()
        };
        assert_eq!(
            session
                .execute(&image, &empty_svd(), "test", warm)
                .unwrap()
                .return_value,
            2
        );
        assert_eq!(session.byte(&image, 0x1010), Some(0));
    }

    #[test]
    fn ownership_conflicts_and_immutable_writes_fail_closed() {
        let image = tiny_image(
            vec![
                0x23, 0xa0, 0xa5, 0x00, // sw a0, 0(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0, 0, 0, 0,
            ],
            12,
        );
        let range = MemoryRange {
            start: 0x1008,
            length: 4,
        };
        let immutable = Scenario {
            arguments: vec![1, range.start],
            memory_ownership: vec![MemoryOwnership {
                range,
                owner: MemoryOwner::Immutable,
            }],
            ..Scenario::default()
        };
        let error = execute(&image, &empty_svd(), "test", immutable).unwrap_err();
        assert!(error.to_string().contains("immutable RAM"));

        let mut session = ExecutionSession::default();
        session
            .execute(
                &image,
                &empty_svd(),
                "test",
                Scenario {
                    arguments: vec![1, range.start],
                    memory_ownership: vec![MemoryOwnership {
                        range,
                        owner: MemoryOwner::Cpu,
                    }],
                    ..Scenario::default()
                },
            )
            .unwrap();
        let conflict = session
            .execute(
                &image,
                &empty_svd(),
                "test",
                Scenario {
                    arguments: vec![1, range.start],
                    memory_ownership: vec![MemoryOwnership {
                        range,
                        owner: MemoryOwner::Dma,
                    }],
                    ..Scenario::default()
                },
            )
            .unwrap_err();
        assert!(conflict.to_string().contains("conflicting RAM ownership"));
    }

    #[test]
    fn unresolved_external_tail_call_fails_closed() {
        let image = tail_relocation_image(None);
        let inventory = image.coverage_inventory("wrapper").unwrap();
        assert_eq!(inventory.unresolved_edges.len(), 1);
        assert!(inventory.branch_sites.is_empty());

        let svd = SvdMap {
            registers: Vec::new(),
            windows: vec![crate::Window { start: 0, end: 1 }],
        };
        let error = execute(&image, &svd, "wrapper", Scenario::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unresolved external call callee")
        );
    }

    #[test]
    fn poison_memory_and_unseeded_mmio_fail_closed() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let empty_svd = empty_svd();
        let mut machine = Machine::new(&image, &empty_svd, 0x1000, Scenario::default());
        assert!(
            machine
                .read(0x4000_0000, 32)
                .unwrap_err()
                .to_string()
                .contains("poison/unmapped")
        );

        let mmio_svd = SvdMap {
            registers: Vec::new(),
            windows: vec![crate::Window {
                start: 0x2010_0000,
                end: 0x2020_0000,
            }],
        };
        let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
        assert!(
            machine
                .read(0x2010_0010, 32)
                .unwrap_err()
                .to_string()
                .contains("no explicit seed or response")
        );
    }

    #[test]
    fn mmio_write_does_not_create_a_generic_readback_value() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let mmio_svd = SvdMap {
            registers: Vec::new(),
            windows: vec![crate::Window {
                start: 0x2010_0000,
                end: 0x2020_0000,
            }],
        };
        let address = 0x2010_0010;

        let mut seeded = Scenario::default();
        seeded.mmio_initial.insert(address, 0x1122_3344);
        let mut machine = Machine::new(&image, &mmio_svd, 0x1000, seeded);
        machine.write(address, 32, 0xaabb_ccdd).unwrap();
        assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);

        let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
        machine.write(address, 32, 0xaabb_ccdd).unwrap();
        assert!(machine.read(address, 32).is_err());
    }

    #[test]
    fn bss_tail_is_known_zero() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
        assert_eq!(image.byte(0x1004), Some(0));
        assert_eq!(image.byte(0x1008), None);
    }

    #[test]
    fn writes_to_read_only_elf_memory_fail_closed() {
        let mut image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
        image.segments[0].writable = false;
        let svd = empty_svd();
        let mut machine = Machine::new(&image, &svd, 0x1000, Scenario::default());

        let error = machine.write(0x1004, 8, 0x5a).unwrap_err();
        assert!(error.to_string().contains("read-only ELF memory"));
        assert!(machine.persistent_memory().is_empty());
    }

    #[test]
    fn execution_rejects_extra_arguments_and_unconsumed_mmio_reads() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let too_many = Scenario {
            arguments: vec![0; 9],
            ..Scenario::default()
        };
        assert!(
            execute(&image, &empty_svd(), "test", too_many)
                .unwrap_err()
                .to_string()
                .contains("stack arguments are not implemented")
        );

        let mut unconsumed = Scenario::default();
        unconsumed
            .mmio_reads
            .entry(0x2010_0010)
            .or_default()
            .push_back(1);
        assert!(
            execute(&image, &empty_svd(), "test", unconsumed)
                .unwrap_err()
                .to_string()
                .contains("unconsumed MMIO read responses")
        );
    }

    #[test]
    fn fence_is_an_ordered_execution_event() {
        let image = tiny_image(
            vec![
                0x0f, 0x00, 0x30, 0x03, // fence rw, rw
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            8,
        );
        let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
        assert_eq!(
            result.events,
            vec![ExecutionEvent::Fence {
                fm: 0,
                predecessor: 3,
                successor: 3,
            }]
        );
    }

    #[test]
    fn reports_only_observed_memory_mutations() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let svd = SvdMap::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("svd/esp32s31-radio.svd"),
        )
        .unwrap();
        let address = 0x3fff_0000;
        let mut scenario = Scenario::default();
        scenario.memory_initial.insert(address, 0xaa);
        scenario.memory_initial.insert(address + 1, 0);
        scenario.observed_memory.push(MemoryRange {
            start: address,
            length: 2,
        });
        let mut machine = Machine::new(&image, &svd, 0, scenario);
        machine.write(address, 16, 0x55aa).unwrap();
        machine.write(address + 8, 8, 0xff).unwrap();
        assert_eq!(
            machine.memory_changes().unwrap(),
            vec![MemoryChange {
                address: address + 1,
                before: 0,
                after: 0x55,
            }]
        );
    }

    #[test]
    fn observed_memory_alias_reports_normalized_addresses() {
        let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
        let svd = SvdMap::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("svd/esp32s31-radio.svd"),
        )
        .unwrap();
        let actual = 0x3fff_0120;
        let mut scenario = Scenario::default();
        scenario.memory_initial.insert(actual, 0);
        scenario.memory_initial.insert(actual + 1, 0);
        scenario.memory_aliases.push(MemoryAlias {
            start: actual,
            length: 2,
            comparison_start: 0,
        });
        let mut machine = Machine::new(&image, &svd, 0, scenario);
        machine.write(actual + 1, 8, 0x5a).unwrap();
        assert_eq!(
            machine.memory_changes().unwrap(),
            vec![MemoryChange {
                address: 1,
                before: 0,
                after: 0x5a,
            }]
        );
    }
}
