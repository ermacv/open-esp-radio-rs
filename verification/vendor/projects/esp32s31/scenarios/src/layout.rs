//! Guest addresses and peripheral encodings shared by the ESP32-S31 scenarios.
//!
//! These are chip and ROM facts owned by this verification package and scratch
//! placements chosen for its requests. Expected values never derive from the
//! compared outputs.
use blobray_domain::{
    CommandBank, CommandCell, CommandPort, DeviceBehavior, DeviceDeclaration, ReadRun,
    RegionLifetime, RegisterCell,
};

/// Input index of the authenticated ROM ELF in every PHY session.
pub const ROM_INPUT: u64 = 1;
/// Input index of the PHY SDK firmware after archive, ROM and production.
pub const PHY_SDK_INPUT: u64 = 3;

/// Caller-owned scratch RAM for probe inputs and copied parameter images.
pub const INPUT: u32 = 0x3fff_0000;
/// Caller-owned scratch RAM for selected outputs.
pub const OUTPUT: u32 = 0x3fff_1000;
/// Explicit callback table referenced through the ROM interface pointer.
pub const CALLBACK_TABLE: u32 = 0x3fff_3000;
/// Source of a 492-byte parameter image copied by captured `memcpy`.
pub const PARAMETER_SOURCE: u32 = 0x3fff_5000;
/// Production-side destination of a copied parameter image.
pub const PARAMETER_DESTINATION: u32 = 0x3fff_6000;
/// Production-side copy of a parameter image in scratch input RAM.
pub const PARAMETER_COPY: u32 = INPUT + 0x800;
/// Size of the captured `phy_param` object.
pub const PHY_PARAM_BYTES: u32 = 492;

/// ROM storage used in `match` patterns, so it is not resolved at run time.
/// Each constant names its pinned-ROM symbol; every session that captures
/// the ROM checks them against its inventory (`verify_rom_symbols`).
///
/// ROM interface-table pointer `rom_phyFuns`, in storage without a PT_LOAD
/// mapping.
pub const ROM_INTERFACE_POINTER: u32 = 0x2f07_fc3c;
/// ROM pointer `phy_param_rom` to the active parameter object.
pub const ROM_PARAMETER_POINTER: u32 = 0x2f07_fc40;
/// ROM callback table `g_phyFuns_instance` installed by `phy_get_romfunc_addr`.
pub const ROM_CALLBACK_TABLE: u32 = 0x2f07_f944;
/// Word slots of `g_phyFuns_instance`.
pub const ROM_CALLBACK_SLOTS: u32 = 13;
pub const ROM_SYMBOLS: [(&str, u32, u64); 3] = [
    ("rom_phyFuns", ROM_INTERFACE_POINTER, 4),
    ("phy_param_rom", ROM_PARAMETER_POINTER, 4),
    (
        "g_phyFuns_instance",
        ROM_CALLBACK_TABLE,
        4 * ROM_CALLBACK_SLOTS as u64,
    ),
];

/// Address of one word slot of the ROM callback table.
pub const fn callback_slot(index: u32) -> u32 {
    assert!(index < ROM_CALLBACK_SLOTS);
    ROM_CALLBACK_TABLE + 4 * index
}
/// Callback-table slot observed after the captured installer runs.
pub const INSTALLED_CALLBACK_SLOT: u32 = callback_slot(10);

/// Packed-command ports of the two analog I2C hosts.
pub const I2C_PORT_0: u32 = 0x2010_f800;
pub const I2C_PORT_1: u32 = 0x2010_f804;
pub const I2C_PORTS: [u32; 2] = [I2C_PORT_0, I2C_PORT_1];
/// Complemented read-mask control of the analog I2C transport.
pub const I2C_READ_MASK: u32 = 0x2010_f81c;
/// Host-map control of the analog I2C transport.
pub const I2C_HOST_MAP: u32 = 0x2010_f820;
/// Packed analog command fields.
pub const COMMAND_SELECTOR_MASK: u32 = 0xffff;
pub const COMMAND_DATA_MASK: u32 = 0x00ff_0000;
pub const COMMAND_READ: u32 = 0x0400_0000;
pub const COMMAND_WRITE: u32 = 0x0500_0000;
pub const COMMAND_BUSY: u32 = 0x0200_0000;

/// Frequency-control word of the channel switch.
pub const FREQUENCY_CONTROL: u32 = 0x2010_001c;
/// Gain-bank base read that starts TX gain publication.
pub const GAIN_BASE: u32 = 0x2010_0408;
/// Baseband work mode; 2 selects the settle branch.
pub const WORK_MODE: u32 = 0x2010_9c18;
/// Low-power temperature-sensor code.
pub const TEMPERATURE_CODE: u32 = 0x2081_8000;
/// Channel readiness status; bit 8 reports a completed frequency switch.
pub const CHANNEL_STATUS: u32 = 0x2010_0028;
pub const CHANNEL_READY: u32 = 0x100;
/// PBus transaction status polled after each PBus command.
pub const PBUS_STATUS: u32 = 0x2010_0890;
/// Gain-memory index/control word and its three data words.
pub const GAIN_INDEX: u32 = 0x2010_0844;
pub const GAIN_DATA: [u32; 3] = [0x2010_0848, 0x2010_084c, 0x2010_0850];

/// Analog I2C command RAM: 45 aligned command words written by the PHY.
pub const COMMAND_RAM: u32 = 0x2010_fc00;

/// Placement of linked vendor images.
pub const IMAGE_CODE_START: u32 = 0x1100_0000;
pub const IMAGE_DATA_START: u32 = 0x2000_0000;
pub const IMAGE_REGION_BYTES: u64 = 0x100_0000;

/// Execution stack of both implementations.
pub const STACK_ADDRESS: u32 = 0x3ffe_0000;
pub const STACK_BYTES: u32 = 0x8000;
/// Stack and peripheral fill patterns exercised by every matrix.
pub const FILLS: [u8; 2] = [0x5a, 0xa5];

/// A fill byte replicated into every byte of a register word.
pub fn fill_word(fill: u8) -> u32 {
    u32::from(fill) * 0x0101_0101
}

/// Event capacity of one phase in the finite matrices.
pub const MAX_EVENTS: u32 = 32768;

/// Analog command bank over both I2C ports with a shared retained cell set.
pub fn analog_bank(
    id: &str,
    applicability: &str,
    busy: u32,
    initial_busy: [u32; 2],
    mut cells: Vec<CommandCell>,
) -> DeviceDeclaration {
    cells.sort_by_key(|c| c.selector);
    DeviceDeclaration {
        id: id.into(),
        applicability: applicability.into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::CommandBank(CommandBank {
            selector_mask: COMMAND_SELECTOR_MASK,
            data_mask: COMMAND_DATA_MASK,
            read_command: COMMAND_READ,
            write_command: COMMAND_WRITE,
            busy_mask: COMMAND_BUSY,
            reset_command: Some(COMMAND_READ),
            ports: I2C_PORTS
                .iter()
                .zip(initial_busy)
                .map(|(address, initial_busy_reads)| CommandPort {
                    address: *address,
                    initial: 0,
                    initial_busy_reads,
                    busy_reads: busy,
                })
                .collect(),
            cells,
        }),
    }
}

/// Radio MMIO block of the PHY, modem and analog I2C master registers.
pub const RADIO_MMIO: u32 = 0x2010_0000;
pub const RADIO_MMIO_BYTES: u32 = 0x1_0000;

/// Every radio register not given an explicit model is retained storage that
/// starts with the fill pattern. Explicit models inside the block take
/// precedence; reads of never-written words stay visible as MMIO events.
pub fn radio_aperture(fill: u8) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "radio-retained".into(),
        applicability: "unmodeled radio registers retain writes; fill-pattern initial words".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RetainedAperture {
            start: RADIO_MMIO,
            length: RADIO_MMIO_BYTES,
            initial: fill_word(fill),
        },
    }
}

/// Retained word registers with explicit initial values.
pub fn register_bank(id: &str, applicability: &str, cells: Vec<(u32, u32)>) -> DeviceDeclaration {
    let mut cells = cells;
    cells.sort();
    DeviceDeclaration {
        id: id.into(),
        applicability: applicability.into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: cells
                .into_iter()
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .collect(),
        },
    }
}

/// A word register that always reads `value`.
pub fn constant_read(id: &str, address: u32, value: u32) -> DeviceDeclaration {
    DeviceDeclaration {
        id: id.into(),
        applicability: "explicit constant peripheral observation".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::ConstantRead {
            address,
            width: 4,
            value,
        },
    }
}

/// A word register that returns finite runs of values, then is exhausted.
pub fn sequence_read(id: &str, address: u32, runs: Vec<ReadRun>) -> DeviceDeclaration {
    DeviceDeclaration {
        id: id.into(),
        applicability: "finite caller-supplied peripheral observations".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::SequenceRead {
            address,
            width: 4,
            runs,
        },
    }
}
