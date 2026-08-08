//! Reviewed state-footprint ownership and access validation.

use crate::{Result, execution};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateAccess {
    Read,
    Write,
    ReadWrite,
}

impl StateAccess {
    const fn permits(self, requested: Self) -> bool {
        matches!(
            (self, requested),
            (Self::ReadWrite, _) | (Self::Read, Self::Read) | (Self::Write, Self::Write)
        )
    }
}

/// One reviewed part of a vendor state image used by a semantic contract.
///
/// Ranges are deliberately expressed relative to the vendor image. Their
/// names describe why the bytes are allowed to participate in the contract;
/// an access that matches no range fails verification instead of silently
/// disappearing from the canonical projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateFootprintRange {
    pub offset: u32,
    pub length: u32,
    pub access: StateAccess,
    pub owner: execution::MemoryOwner,
    pub name: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StateFootprintStats {
    pub read_bytes: usize,
    pub written_bytes: usize,
    pub classified_ranges: usize,
}

pub(super) fn validate_state_footprint(
    contract: &str,
    result: &execution::ExecutionResult,
    state_base: u32,
    state_length: u32,
    ranges: &[StateFootprintRange],
) -> Result<StateFootprintStats> {
    let state_end = state_base
        .checked_add(state_length)
        .ok_or("semantic state range overflows RV32")?;
    let mut reads = std::collections::BTreeSet::new();
    let mut writes = std::collections::BTreeSet::new();
    let mut unknown_reads = std::collections::BTreeSet::new();
    let mut unknown_writes = std::collections::BTreeSet::new();

    let mut classify = |address: u32, width: u8, access: StateAccess| {
        for byte in 0..u32::from(width / 8) {
            let address = address.wrapping_add(byte);
            if !(state_base..state_end).contains(&address) {
                continue;
            }
            let offset = address - state_base;
            let permitted = ranges.iter().any(|range| {
                offset
                    .checked_sub(range.offset)
                    .is_some_and(|relative| relative < range.length)
                    && range.access.permits(access)
            });
            match access {
                StateAccess::Read => {
                    reads.insert(offset);
                    if !permitted {
                        unknown_reads.insert(offset);
                    }
                }
                StateAccess::Write => {
                    writes.insert(offset);
                    if !permitted {
                        unknown_writes.insert(offset);
                    }
                }
                StateAccess::ReadWrite => unreachable!("timeline access has one direction"),
            }
        }
    };
    for event in &result.timeline {
        match event {
            execution::ExecutionTimelineEvent::RamRead { width, address, .. } => {
                classify(*address, *width, StateAccess::Read);
            }
            execution::ExecutionTimelineEvent::RamWrite { width, address, .. } => {
                classify(*address, *width, StateAccess::Write);
            }
            _ => {}
        }
    }
    if !unknown_reads.is_empty() || !unknown_writes.is_empty() {
        let offsets = |values: &std::collections::BTreeSet<u32>| {
            values
                .iter()
                .map(|offset| format!("{offset:#05x}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        return Err(format!(
            "{contract} accessed unclassified state: reads=[{}] writes=[{}]",
            offsets(&unknown_reads),
            offsets(&unknown_writes)
        )
        .into());
    }
    Ok(StateFootprintStats {
        read_bytes: reads.len(),
        written_bytes: writes.len(),
        classified_ranges: ranges.len(),
    })
}

pub(super) const RF_INIT_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x002,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "pbus-rx-path",
    },
    StateFootprintRange {
        offset: 0x016,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "rf-init-control-016",
    },
    StateFootprintRange {
        offset: 0x04a,
        length: 1,
        access: StateAccess::Write,
        owner: execution::MemoryOwner::MmioDerived,
        name: "bbpll-register-snapshot",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "xtal-frequency-code",
    },
    StateFootprintRange {
        offset: 0x0a4,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::Cpu,
        name: "calibration-completion-flags",
    },
    StateFootprintRange {
        offset: 0x0e8,
        length: 9,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "filter-dcap-state",
    },
    StateFootprintRange {
        offset: 0x18e,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "rfpll-parameter-18e",
    },
    StateFootprintRange {
        offset: 0x193,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "channel-frequency-override",
    },
    StateFootprintRange {
        offset: 0x19e,
        length: 3,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "xtal-duty-result",
    },
    StateFootprintRange {
        offset: 0x1ac,
        length: 2,
        access: StateAccess::Write,
        owner: execution::MemoryOwner::MmioDerived,
        name: "rf-init-calibration-scratch",
    },
    StateFootprintRange {
        offset: 0x1af,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "front-end-control",
    },
];

pub(super) const CHANNEL_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x000,
        length: 2,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "temperature-result",
    },
    StateFootprintRange {
        offset: 0x007,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-publication-control",
    },
    StateFootprintRange {
        offset: 0x016,
        length: 1,
        access: StateAccess::Write,
        owner: execution::MemoryOwner::MmioDerived,
        name: "temperature-sensor-range",
    },
    StateFootprintRange {
        offset: 0x020,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "frequency-offset",
    },
    StateFootprintRange {
        offset: 0x026,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "channel-14-mic-control",
    },
    StateFootprintRange {
        offset: 0x028,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "dot11p-configuration",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "crystal-selector",
    },
    StateFootprintRange {
        offset: 0x0a8,
        length: 24,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-seed",
    },
    StateFootprintRange {
        offset: 0x0d0,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-configuration",
    },
    StateFootprintRange {
        offset: 0x0dc,
        length: 6,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-capacitance",
    },
    StateFootprintRange {
        offset: 0x0f1,
        length: 7,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-curve-and-correction",
    },
    StateFootprintRange {
        offset: 0x11c,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::Cpu,
        name: "channel-result",
    },
    StateFootprintRange {
        offset: 0x123,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-base",
    },
];

pub(super) fn declare_state_ownership(
    scenario: &mut execution::Scenario,
    state_base: u32,
    ranges: &[StateFootprintRange],
) {
    scenario
        .memory_ownership
        .extend(ranges.iter().map(|range| execution::MemoryOwnership {
            range: crate::execution_model::MemoryRange {
                start: state_base + range.offset,
                length: range.length,
            },
            owner: range.owner,
        }));
}

pub fn vendor_rf_init_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-rf-init",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
        RF_INIT_STATE_FOOTPRINT,
    )
}

pub fn vendor_channel_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-channel",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
        CHANNEL_STATE_FOOTPRINT,
    )
}
