//! Guest addresses and peripheral encodings shared by the ESP32-S31 scenarios.
//!
//! These are chip and ROM facts owned by this verification package and scratch
//! placements chosen for its requests. Expected values never derive from the
//! compared outputs.
use blobray_domain::{
    CommandBank, CommandCell, CommandPort, DeviceBehavior, DeviceDeclaration, RegionLifetime,
};

/// Caller-owned scratch RAM for probe inputs and copied parameter images.
pub const INPUT: u32 = 0x3fff_0000;
/// Caller-owned scratch RAM for selected outputs.
pub const OUTPUT: u32 = 0x3fff_1000;
/// Explicit callback table referenced through the ROM interface pointer.
pub const CALLBACK_TABLE: u32 = 0x3fff_3000;
/// ABI words read by the stack-entry and I2C-entry adapters.
pub const ABI_WORDS: u32 = 0x3fff_4000;
/// Source of a 516-byte parameter image copied by captured `memcpy`.
pub const PARAMETER_SOURCE: u32 = 0x3fff_5000;
/// Production-side destination of a copied parameter image.
pub const PARAMETER_DESTINATION: u32 = 0x3fff_6000;
/// Size of the captured `phy_param` object.
pub const PHY_PARAM_BYTES: u32 = 516;

/// ROM interface-table pointer in captured BSS without a PT_LOAD mapping.
pub const ROM_INTERFACE_POINTER: u32 = 0x2f07_fc3c;
/// ROM pointer to the active parameter object.
pub const ROM_PARAMETER_POINTER: u32 = 0x2f07_fc40;
/// Captured ROM callback table installed by `phy_get_romfunc_addr`.
pub const ROM_CALLBACK_TABLE: u32 = 0x2f07_f944;

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

/// Analog I2C command RAM: 45 aligned command words written by the PHY.
pub const COMMAND_RAM: u32 = 0x2010_fc00;

/// Execution stack of both implementations.
pub const STACK_ADDRESS: u32 = 0x3ffe_0000;
pub const STACK_BYTES: u32 = 0x8000;
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
