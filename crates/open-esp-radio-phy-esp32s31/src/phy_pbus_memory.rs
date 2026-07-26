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
//! two varying bytes explicitly, publishes one finite entry per completion,
//! and returns the six sampled words to the unique cold-state owner. The ROM
//! `memcpy` calls and `phy_param` ABI cell are construction artifacts, not
//! dependencies of the radio algorithm, and are intentionally absent.

pub const PHY_PBUS_MEMORY_DATA_ADDRESS: usize = 0x2010_0848;
pub const PHY_PBUS_MEMORY_COMMAND_ADDRESS: usize = 0x2010_0844;
pub const PHY_PBUS_MEMORY_COMMAND_MASK: u32 = 0x001f_f800;
pub const PHY_PBUS_MEMORY_COMMAND_PRESERVE_MASK: u32 = 0xffe0_07ff;
pub const PHY_PBUS_MEMORY_ENTRY_COUNT: u8 = 60;

pub const PHY_PBUS_MEMORY_SAVED_ADDRESSES: [usize; 6] = [
    0x2010_0854,
    0x2010_0858,
    0x2010_085c,
    0x2010_0860,
    0x2010_0864,
    0x2010_0868,
];

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
pub struct PhyPbusMemoryControlField {
    address: usize,
    mask: u32,
    value: u32,
}

impl PhyPbusMemoryControlField {
    pub const fn address(self) -> usize {
        self.address
    }

    pub const fn mask(self) -> u32 {
        self.mask
    }

    pub const fn value(self) -> u32 {
        self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryEntry {
    group: u8,
    index: u8,
    control: Option<PhyPbusMemoryControlField>,
    data: u32,
    command_bits: u32,
}

impl PhyPbusMemoryEntry {
    pub const fn group(self) -> u8 {
        self.group
    }

    pub const fn index(self) -> u8 {
        self.index
    }

    pub const fn control(self) -> Option<PhyPbusMemoryControlField> {
        self.control
    }

    pub const fn data(self) -> u32 {
        self.data
    }

    pub const fn command_bits(self) -> u32 {
        self.command_bits
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryOutcome {
    pub saved_registers: [u32; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryAction {
    Program(PhyPbusMemoryEntry),
    Capture { addresses: [usize; 6] },
    Complete(PhyPbusMemoryOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusMemoryCompletion {
    Programmed(PhyPbusMemoryEntry),
    Captured {
        addresses: [usize; 6],
        values: [u32; 6],
    },
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
        command_accumulator: u32,
    },
    Capture,
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

const fn control_field(group: u8, header_accumulator: u32) -> PhyPbusMemoryControlField {
    let shift = if group & 1 == 0 { 0 } else { 16 };
    let pair = (group >> 1) as usize;
    let encoded = ((((header_accumulator
        .wrapping_add(group_count(group) as u32)
        .wrapping_sub(1))
        << 8)
        | header_accumulator)
        & 0xffff)
        << shift;
    PhyPbusMemoryControlField {
        address: PHY_PBUS_MEMORY_SAVED_ADDRESSES[pair],
        mask: 0xffff_u32 << shift,
        value: encoded,
    }
}

const fn entry(
    parameters: PhyPbusMemoryParameters,
    group: u8,
    index: u8,
    header_accumulator: u32,
    command_accumulator: u32,
) -> PhyPbusMemoryEntry {
    PhyPbusMemoryEntry {
        group,
        index,
        control: if index == 0 {
            Some(control_field(group, header_accumulator))
        } else {
            None
        },
        data: data_word(parameters, group, index),
        command_bits: command_accumulator & PHY_PBUS_MEMORY_COMMAND_MASK,
    }
}

/// Exact caller-driven replacement for the three-function ROM graph.
///
/// Every `Program` action contains at most one control-register RMW, one data
/// write, and one command-register RMW. `Capture` is six fixed reads. No
/// action contains a retry, readiness sample, delay, callback, or allocation.
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
                command_accumulator: 0x0010_0000,
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
            PhyPbusMemoryStep::Capture => PhyPbusMemoryAction::Capture {
                addresses: PHY_PBUS_MEMORY_SAVED_ADDRESSES,
            },
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
                let next_accumulator = command_accumulator.wrapping_add(0x800);
                if index + 1 != group_count(group) {
                    PhyPbusMemoryStep::Program {
                        group,
                        index: index + 1,
                        header_accumulator,
                        command_accumulator: next_accumulator,
                    }
                } else if group + 1 != GROUP_COUNT {
                    PhyPbusMemoryStep::Program {
                        group: group + 1,
                        index: 0,
                        header_accumulator: next_accumulator,
                        command_accumulator: next_accumulator.wrapping_add(0x200) << 11,
                    }
                } else {
                    PhyPbusMemoryStep::Capture
                }
            }
            (
                PhyPbusMemoryStep::Capture,
                PhyPbusMemoryCompletion::Captured {
                    addresses: PHY_PBUS_MEMORY_SAVED_ADDRESSES,
                    values,
                },
            ) => PhyPbusMemoryStep::Complete(PhyPbusMemoryOutcome {
                saved_registers: values,
            }),
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
}

/// One finite direct-MMIO edge of the PBus-memory initializer.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPbusMemoryMmioBinding {
    action: PhyPbusMemoryAction,
}

impl PhyPbusMemoryMmioBinding {
    pub fn new(action: PhyPbusMemoryAction) -> Result<Self, PhyPbusMemoryBindingError> {
        match action {
            PhyPbusMemoryAction::Program(_) | PhyPbusMemoryAction::Capture { .. } => {
                Ok(Self { action })
            }
            PhyPbusMemoryAction::Complete(_) => Err(PhyPbusMemoryBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyPbusMemoryAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyPbusMemoryCompletion {
        match self.action {
            PhyPbusMemoryAction::Program(entry) => {
                crate::radio_hal::program_phy_pbus_memory_entry(entry);
                PhyPbusMemoryCompletion::Programmed(entry)
            }
            PhyPbusMemoryAction::Capture { addresses } => PhyPbusMemoryCompletion::Captured {
                addresses,
                values: crate::radio_hal::capture_phy_pbus_memory_registers(),
            },
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
        let mut seen_counts = [0_u8; 12];
        let mut seen = 0_u8;
        let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);

        while let PhyPbusMemoryAction::Program(entry) = transition.action() {
            assert_eq!(entry.index(), seen_counts[entry.group() as usize]);
            if entry.index() == 0 {
                assert!(entry.control().is_some());
            } else {
                assert_eq!(entry.control(), None);
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
            PhyPbusMemoryAction::Capture {
                addresses: PHY_PBUS_MEMORY_SAVED_ADDRESSES
            }
        );
    }

    #[test]
    fn recovered_tables_include_all_parameter_derived_overrides() {
        let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);
        let mut selected = [0_u32; 5];

        while let PhyPbusMemoryAction::Program(entry) = transition.action() {
            match (entry.group(), entry.index()) {
                (0, 0) => selected[0] = entry.data(),
                (6, 2) => selected[1] = entry.data(),
                (8, 1) => selected[2] = entry.data(),
                (8, 2) => selected[3] = entry.data(),
                (10, 0) => selected[4] = entry.data(),
                _ => {}
            }
            transition
                .advance(PhyPbusMemoryCompletion::Programmed(entry))
                .unwrap();
        }

        assert_eq!(
            selected,
            [
                0x0009_7fff,
                0x0017_17ff,
                0x0014_fdff,
                0x0480_11ff,
                0x0014_fdff,
            ]
        );
    }

    #[test]
    fn capture_identity_and_output_are_owned() {
        let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);
        while let PhyPbusMemoryAction::Program(entry) = transition.action() {
            transition
                .advance(PhyPbusMemoryCompletion::Programmed(entry))
                .unwrap();
        }

        let values = [1, 2, 3, 4, 5, 6];
        assert_eq!(
            transition.advance(PhyPbusMemoryCompletion::Captured {
                addresses: [0; 6],
                values,
            }),
            Err(PhyPbusMemoryTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyPbusMemoryCompletion::Captured {
                addresses: PHY_PBUS_MEMORY_SAVED_ADDRESSES,
                values,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome {
                saved_registers: values
            })
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
    fn mmio_binding_covers_program_and_capture_but_not_terminal() {
        let transition = PhyPbusMemoryTransition::new(PARAMETERS);
        assert!(matches!(
            PhyPbusMemoryMmioBinding::new(transition.action()),
            Ok(_)
        ));
        assert!(matches!(
            PhyPbusMemoryMmioBinding::new(PhyPbusMemoryAction::Capture {
                addresses: PHY_PBUS_MEMORY_SAVED_ADDRESSES,
            }),
            Ok(_)
        ));
        assert_eq!(
            PhyPbusMemoryMmioBinding::new(PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome {
                saved_registers: [0; 6],
            })),
            Err(PhyPbusMemoryBindingError::UnsupportedAction)
        );
    }
}
