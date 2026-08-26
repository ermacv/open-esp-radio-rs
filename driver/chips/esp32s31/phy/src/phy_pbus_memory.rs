//! Rust-owned ESP32-S31 PHY PBus-memory initialization.
//!
//! The reference is the complete rev0 ROM graph
//! `phy_set_pbus_mem` (`0x2f82_479e`, 384 bytes),
//! `phy_write_pbus_mem` (`0x2f82_4634`, 362 bytes), and
//! `phy_save_pbus_reg` (`0x2f82_4602`, 50 bytes). The ROM parent constructs
//! twelve tables on its stack, publishes 60 entries, then stores six MMIO
//! words through the global `phy_param` pointer.
//!
//! Rust keeps the recovered constants in read-only storage, takes the only
//! two varying bytes explicitly, and publishes one finite entry per
//! completion. `phy_save_pbus_reg` only copied six ordinary register images
//! into ROM's private `phy_param` buffer; no open runtime or restore path
//! consumes them. That dead copy, its reads, the ROM `memcpy` calls, and the
//! `phy_param` ABI cell are intentionally absent.

pub const PHY_PBUS_MEMORY_ENTRY_COUNT: u8 = 60;

const GROUP_COUNT: u8 = 12;

const TABLE_A: [u32; 8] = [
    0x0004_01ff,
    0x0008_01ff,
    0x0014_01ff,
    0x0018_01ff,
    0x0048_01ff,
    0,
    0,
    0,
];

const TABLE_B: [u32; 8] = [
    0x0005_01ff,
    0x0014_f9ff,
    0x0048_01ff,
    0x0044_17ff,
    0x00f0_0000,
    0x00f1_0000,
    0x00f2_0000,
    0x00f4_0000,
];

const TABLE_C: [u32; 8] = [
    0x0004_01ff,
    0x0048_01ff,
    0x0014_01ff,
    0x0018_01ff,
    0x0044_01ff,
    0,
    0,
    0,
];

const TABLE_D: [u32; 8] = [0x0014_f9ff, 0x0057_81ff, 0x00f3_0000, 0, 0, 0, 0, 0];

const TABLE_E: [u32; 8] = [0x0014_fdff, 0x0057_81ff, 0x00f3_0000, 0, 0, 0, 0, 0];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryParameters {
    pub parameter_002: u8,
    pub parameter_014: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryGroupBoundary {
    group: u8,
    first_entry: u8,
    last_entry: u8,
}

impl PhyPbusMemoryGroupBoundary {
    pub const fn group(self) -> u8 {
        self.group
    }

    pub const fn first_entry(self) -> u8 {
        self.first_entry
    }

    pub const fn last_entry(self) -> u8 {
        self.last_entry
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryEntry {
    group: u8,
    index: u8,
    boundary: Option<PhyPbusMemoryGroupBoundary>,
    data: u32,
    memory_command: u16,
}

impl PhyPbusMemoryEntry {
    pub const fn group(self) -> u8 {
        self.group
    }

    pub const fn index(self) -> u8 {
        self.index
    }

    pub const fn boundary(self) -> Option<PhyPbusMemoryGroupBoundary> {
        self.boundary
    }

    pub const fn data(self) -> u32 {
        self.data
    }

    pub const fn memory_command(self) -> u16 {
        self.memory_command
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryOutcome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryAction {
    Program(PhyPbusMemoryEntry),
    Complete(PhyPbusMemoryOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryCompletion {
    Programmed(PhyPbusMemoryEntry),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPbusMemoryStep {
    Program {
        group: u8,
        index: u8,
        header_accumulator: u32,
        command_accumulator: u16,
    },
    Complete(PhyPbusMemoryOutcome),
}

const fn group_count(group: u8) -> u8 {
    match group {
        0 | 2 | 6 | 8 => 8,
        1 | 3 | 7 | 9 => 5,
        4 | 10 => 3,
        _ => 1,
    }
}

const fn word_at(words: [u32; 8], index: u8) -> u32 {
    match index {
        0 => words[0],
        1 => words[1],
        2 => words[2],
        3 => words[3],
        4 => words[4],
        5 => words[5],
        6 => words[6],
        _ => words[7],
    }
}

const fn first_table(parameter_002: u8) -> [u32; 8] {
    [
        ((parameter_002 as u32) << 9) | 0x0008_01ff,
        0x0044_01ff,
        0x0017_13ff,
        0x0048_03ff,
        0x0054_01ff,
        0x0004_87ff,
        0x00f5_0000,
        0x00f6_0000,
    ]
}

const fn data_word(parameters: PhyPbusMemoryParameters, group: u8, index: u8) -> u32 {
    match group {
        0 => word_at(first_table(parameters.parameter_002), index),
        1 | 7 => word_at(TABLE_A, index),
        2 => word_at(TABLE_B, index),
        3 | 9 => word_at(TABLE_C, index),
        4 => word_at(TABLE_D, index),
        5 | 11 => 0x0054_01ff,
        6 => {
            if index == 2 {
                0x0017_17ff
            } else {
                word_at(first_table(parameters.parameter_002), index)
            }
        }
        8 => match index {
            1 => 0x0014_fdff,
            2 => ((parameters.parameter_014 as u32) << 12) | 0x0480_01ff,
            _ => word_at(TABLE_B, index),
        },
        10 => word_at(TABLE_E, index),
        _ => word_at(TABLE_D, index),
    }
}

const fn group_boundary(group: u8, header_accumulator: u32) -> PhyPbusMemoryGroupBoundary {
    PhyPbusMemoryGroupBoundary {
        group,
        first_entry: header_accumulator as u8,
        last_entry: header_accumulator
            .wrapping_add(group_count(group) as u32)
            .wrapping_sub(1) as u8,
    }
}

const fn entry(
    parameters: PhyPbusMemoryParameters,
    group: u8,
    index: u8,
    header_accumulator: u32,
    command_accumulator: u16,
) -> PhyPbusMemoryEntry {
    PhyPbusMemoryEntry {
        group,
        index,
        boundary: if index == 0 {
            Some(group_boundary(group, header_accumulator))
        } else {
            None
        },
        data: data_word(parameters, group, index),
        memory_command: command_accumulator,
    }
}

/// Exact caller-driven replacement for the live PBUS programming graph.
///
/// Every `Program` action contains at most one control-register RMW, one data
/// write, and one command-register RMW. No action contains a retry, readiness
/// sample, delay, callback, or allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryTransition {
    parameters: PhyPbusMemoryParameters,
    step: PhyPbusMemoryStep,
}

impl PhyPbusMemoryTransition {
    pub const fn new(parameters: PhyPbusMemoryParameters) -> Self {
        Self {
            parameters,
            step: PhyPbusMemoryStep::Program {
                group: 0,
                index: 0,
                header_accumulator: 0,
                command_accumulator: 0x200,
            },
        }
    }

    pub const fn action(self) -> PhyPbusMemoryAction {
        match self.step {
            PhyPbusMemoryStep::Program {
                group,
                index,
                header_accumulator,
                command_accumulator,
            } => PhyPbusMemoryAction::Program(entry(
                self.parameters,
                group,
                index,
                header_accumulator,
                command_accumulator,
            )),
            PhyPbusMemoryStep::Complete(outcome) => PhyPbusMemoryAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPbusMemoryCompletion,
    ) -> Result<(), PhyPbusMemoryTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyPbusMemoryStep::Program {
                    group,
                    index,
                    header_accumulator,
                    command_accumulator,
                },
                PhyPbusMemoryCompletion::Programmed(completed),
            ) if completed
                == entry(
                    self.parameters,
                    group,
                    index,
                    header_accumulator,
                    command_accumulator,
                ) =>
            {
                let next_accumulator = command_accumulator.wrapping_add(1);
                if index + 1 != group_count(group) {
                    PhyPbusMemoryStep::Program {
                        group,
                        index: index + 1,
                        header_accumulator,
                        command_accumulator: next_accumulator,
                    }
                } else if group + 1 != GROUP_COUNT {
                    // `header_accumulator` is the unshifted PBUS-memory entry
                    // number stored in the six group-boundary registers.
                    // `command_accumulator` is that address plus the 0x200
                    // command-space base. The HAL owns its recovered bit
                    // position, so the transition retains only the semantic
                    // ten-bit command value.
                    PhyPbusMemoryStep::Program {
                        group: group + 1,
                        index: 0,
                        header_accumulator: header_accumulator
                            .wrapping_add(group_count(group) as u32),
                        command_accumulator: next_accumulator,
                    }
                } else {
                    PhyPbusMemoryStep::Complete(PhyPbusMemoryOutcome)
                }
            }
            (PhyPbusMemoryStep::Complete(_), _) => {
                return Err(PhyPbusMemoryTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPbusMemoryTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryBindingError {
    UnsupportedAction,
    InvalidGroup,
    InvalidCommand,
}

/// One finite direct-MMIO edge of the PBus-memory initializer.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryMmioBinding {
    action: PhyPbusMemoryAction,
}

impl PhyPbusMemoryMmioBinding {
    pub fn new(action: PhyPbusMemoryAction) -> Result<Self, PhyPbusMemoryBindingError> {
        match action {
            PhyPbusMemoryAction::Program(_) => Ok(Self { action }),
            PhyPbusMemoryAction::Complete(_) => Err(PhyPbusMemoryBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyPbusMemoryAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<PhyPbusMemoryCompletion, PhyPbusMemoryBindingError> {
        match self.action {
            PhyPbusMemoryAction::Program(entry) => {
                let boundary = entry.boundary().map(|field| {
                    open_esp_radio_esp32s31_hal::phy_memory::PbusMemoryGroupBoundary {
                        group: field.group(),
                        first_entry: field.first_entry(),
                        last_entry: field.last_entry(),
                    }
                });
                open_esp_radio_esp32s31_hal::phy_memory::program_pbus_memory_entry(
                    registers,
                    boundary,
                    entry.data(),
                    entry.memory_command(),
                )
                .map_err(|error| match error {
                    open_esp_radio_esp32s31_hal::phy_memory::PhyMemoryError::PbusGroupOutOfRange => {
                        PhyPbusMemoryBindingError::InvalidGroup
                    }
                    open_esp_radio_esp32s31_hal::phy_memory::PhyMemoryError::PbusCommandOutOfRange => {
                        PhyPbusMemoryBindingError::InvalidCommand
                    }
                })?;
                Ok(PhyPbusMemoryCompletion::Programmed(entry))
            }
            PhyPbusMemoryAction::Complete(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: PhyPbusMemoryParameters = PhyPbusMemoryParameters {
        parameter_002: 0xbf,
        parameter_014: 1,
    };

    #[test]
    fn exact_sixty_entry_rom_sequence_is_explicit() {
        let expected_counts = [8_u8, 5, 8, 5, 3, 1, 8, 5, 8, 5, 3, 1];
        let expected_first = [0_u8, 8, 13, 21, 26, 29, 30, 38, 43, 51, 56, 59];
        let expected_last = [7_u8, 12, 20, 25, 28, 29, 37, 42, 50, 55, 58, 59];
        let mut seen_counts = [0_u8; 12];
        let mut seen = 0_u8;
        let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);

        while let PhyPbusMemoryAction::Program(entry) = transition.action() {
            assert_eq!(entry.index(), seen_counts[entry.group() as usize]);
            if entry.index() == 0 {
                let control = entry.boundary().unwrap();
                assert_eq!(control.group(), entry.group());
                assert_eq!(
                    control.first_entry(),
                    expected_first[entry.group() as usize]
                );
                assert_eq!(control.last_entry(), expected_last[entry.group() as usize]);
            } else {
                assert_eq!(entry.boundary(), None);
            }
            seen_counts[entry.group() as usize] += 1;
            seen += 1;
            transition
                .advance(PhyPbusMemoryCompletion::Programmed(entry))
                .unwrap();
        }

        assert_eq!(seen, PHY_PBUS_MEMORY_ENTRY_COUNT);
        assert_eq!(seen_counts, expected_counts);
        assert_eq!(
            transition.action(),
            PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome)
        );
    }

    #[test]
    fn completion_must_match_the_exact_entry() {
        let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);
        let PhyPbusMemoryAction::Program(first) = transition.action() else {
            panic!("first action must program");
        };
        let mut other = PhyPbusMemoryTransition::new(PhyPbusMemoryParameters {
            parameter_002: 0,
            parameter_014: 1,
        });
        let PhyPbusMemoryAction::Program(wrong) = other.action() else {
            panic!("first action must program");
        };
        assert_eq!(
            transition.advance(PhyPbusMemoryCompletion::Programmed(wrong)),
            Err(PhyPbusMemoryTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyPbusMemoryCompletion::Programmed(first))
            .unwrap();
        assert!(matches!(
            transition.action(),
            PhyPbusMemoryAction::Program(_)
        ));
        let _ = &mut other;
    }

    #[test]
    fn mmio_binding_accepts_program_but_not_terminal_state() {
        let transition = PhyPbusMemoryTransition::new(PARAMETERS);
        assert!(PhyPbusMemoryMmioBinding::new(transition.action()).is_ok());
        assert_eq!(
            PhyPbusMemoryMmioBinding::new(PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome)),
            Err(PhyPbusMemoryBindingError::UnsupportedAction)
        );
    }
}
