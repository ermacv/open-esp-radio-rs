//! The analog-register I2C transport of the pinned ESP32-C5 `libphy.a`
//! (`phy_i2c.o`) against the compiled production transport.
//!
//! Each leaf runs the vendor function and the production probe over the
//! same argument domains, the radio register fills and an analog command
//! bank that answers the one register the case addresses. Every register
//! effect compares exactly except the completion polling production adds
//! before a read, and each case also meets the expectation below, which
//! derives from the reviewed transport facts and neither execution.
//!
//! SOURCE: `esp32c5/libphy.a[phy_i2c.o]` of `esp-phy-lib` 20f1db05
//! (evidence `C5_BLOB_LIBPHY_PHY_I2C`), with the host and read-mask roles
//! of ESP-IDF 4d59230d `regi2c_impl.c` and `i2c_ana_mst_reg.h` (evidence
//! `ESP_IDF_4D59230D_C5_I2C_ANA_MST`).
use crate::{I2C_HOST_MAP, I2C_PORTS, I2C_READ_MASK, LIBRARY, ROM};
use blobray_domain::{CommandCell, EffectRule};
use oer_vendor_scenario_engine::evidence::PhyEffect;
use oer_vendor_scenario_engine::harness::Result;
use oer_vendor_scenario_engine::leaf::{
    Domain, Leaf, Objects, Observed, Suite, Vendor, expected, leaf, objects, ruled, stated,
};
use oer_vendor_scenario_engine::phy::contracts::port_polling;
use oer_vendor_scenario_engine::phy::layout::{COMMAND_READ, COMMAND_WRITE, analog_bank};

/// Every analog block the transport admits.
const BLOCKS: &[u32] = &[
    0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f,
];
/// Every value of the vendor's `uint8_t` block argument.
const BLOCK_BYTES: [u32; 256] = {
    let mut values = [0; 256];
    let mut index = 0;
    while index < values.len() {
        values[index] = index as u32;
        index += 1;
    }
    values
};
/// The first and last block, the aliased block and a host-one block, for
/// the field leaves.
const FIELD_BLOCKS: &[u32] = &[0x61, 0x62, 0x66, 0x6f];
/// Host arguments, which both sides ignore.
const HOSTS: &[u32] = &[0, 1];
const HOST: &[u32] = &[0];
/// Registers: the first, an inner and the last.
const REGISTERS: &[u32] = &[0x00, 0x07, 0xff];
/// Written bytes and sampled register values: clear, a pattern and set.
const BYTES: &[u32] = &[0x00, 0x5a, 0xff];
const SAMPLES: &[u32] = &[0x00, 0xa5, 0xff];
/// Field bounds: whole byte, upper and lower nibble-sized fields.
const MOST_SIGNIFICANT: &[u32] = &[7, 4];
const LEAST_SIGNIFICANT: &[u32] = &[0, 3];
/// Field values every admitted field holds.
const FIELD_VALUES: &[u32] = &[0, 1];

/// Blocks whose host is one; every other block uses host zero.
const HOST_ONE_BLOCKS: &[u32] = &[0x63, 0x65, 0x66, 0x68, 0x6a, 0x6b, 0x6e];
/// Read-mask bit of each block from 0x61, before its shift by two.
const READ_MASK_BITS: [u32; 15] = [
    0x40, 0, 0x4, 0, 0x400, 0x20, 0x1, 0x8, 0x80, 0x10, 0x2, 0x800, 0x100, 0x200, 0x1000,
];
const READ_MASK_SHIFT: u32 = 2;
/// The host-map bits every host selection rewrites.
const HOST_MAP_KEPT: u32 = 0xfffe_000f;
const HOST_MAP_SET: u32 = 0x0001_9c10;
/// The block the vendor addresses as another slave.
const ALIASED_BLOCK: u32 = 0x62;
const ALIAS_SLAVE: u32 = 0x65;
/// Command fields: register and data positions.
const REGISTER_SHIFT: u32 = 8;
const DATA_SHIFT: u32 = 16;
/// Busy reads each command reports before it completes, and the busy reads
/// of a port no earlier command occupies.
const BUSY_READS: u32 = 1;
const IDLE: u32 = 0;
/// Completion polls one case may perform per port.
const MAX_POLLS: u32 = 64;

fn host(block: u32) -> usize {
    usize::from(HOST_ONE_BLOCKS.contains(&block))
}

/// The read mask of `block`; a block outside the transport has none.
fn read_mask(block: u32) -> u32 {
    block
        .checked_sub(BLOCKS[0])
        .and_then(|index| READ_MASK_BITS.get(index as usize))
        .map_or(0, |bit| bit << READ_MASK_SHIFT)
}

fn slave(block: u32) -> u32 {
    if block == ALIASED_BLOCK {
        ALIAS_SLAVE
    } else {
        block
    }
}

fn selector(block: u32, register: u32) -> u32 {
    register << REGISTER_SHIFT | slave(block)
}

fn field_mask(most: u32, least: u32) -> u32 {
    (1 << (most - least + 1)) - 1
}

/// An analog bank holding `value` in the one register a case addresses,
/// whose addressed port first reports `initial_busy` busy reads.
fn bank(block: u32, register: u32, value: u32, initial_busy: u32) -> Objects {
    Objects {
        devices: vec![analog_bank(
            "analog",
            "the addressed analog register; commands complete after one busy read",
            BUSY_READS,
            {
                let mut ports = [IDLE; 2];
                ports[host(block)] = initial_busy;
                ports
            },
            vec![CommandCell {
                selector: selector(block, register),
                initial: value,
                reads: None,
            }],
        )],
        ..Default::default()
    }
}

/// `(block, host, register, …, sample)`: the vendor takes every word but
/// the sample, which seeds the addressed register.
fn sampled_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (sample, arguments) = words.split_last().expect("words and a sample");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        ..bank(words[0], words[2], *sample, IDLE)
    })
}

/// `(block, host, register, data, port state)` over a cleared register: the
/// port is idle, or still busy with an earlier command, which a write waits
/// for before it issues.
fn written_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (busy, arguments) = words.split_last().expect("words and a port state");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        ..bank(words[0], words[2], 0, *busy)
    })
}

/// Port states of a write: idle, and busy with an earlier command.
const PORT_STATES: &[u32] = &[IDLE, BUSY_READS];

/// Production polls a port for completion before it issues a read; the
/// vendor issues at once.
fn polling() -> Vec<EffectRule> {
    port_polling(MAX_POLLS)
}

/// The host-map read and its rewrite, which every host selection performs.
fn check_host_map(observed: &Observed) -> std::result::Result<(), String> {
    let old = observed
        .effects
        .iter()
        .find_map(|e| match e {
            PhyEffect::Read(address, value) if *address == I2C_HOST_MAP => Some(*value),
            _ => None,
        })
        .ok_or("no host-map read")?;
    let expected = PhyEffect::Write(I2C_HOST_MAP, old & HOST_MAP_KEPT | HOST_MAP_SET);
    if !observed.effects.contains(&expected) {
        return Err(format!("no host-map write {expected:x?}"));
    }
    Ok(())
}

fn check_return(observed: &Observed, value: u32) -> std::result::Result<(), String> {
    if observed.returned != Some(value) {
        return Err(format!(
            "returned {:x?}, expected {value:#x}",
            observed.returned
        ));
    }
    Ok(())
}

/// The command write of one case, with nothing written outside the
/// transport registers.
fn check_command(observed: &Observed, block: u32, command: u32) -> std::result::Result<(), String> {
    let port = I2C_PORTS[host(block)];
    if !observed.effects.contains(&PhyEffect::Write(port, command)) {
        return Err(format!("no command {command:#x} at {port:#x}"));
    }
    let transport = [I2C_PORTS[0], I2C_PORTS[1], I2C_READ_MASK, I2C_HOST_MAP];
    if let Some(stray) = observed
        .effects
        .iter()
        .find(|e| matches!(e, PhyEffect::Write(address, _) if !transport.contains(address)))
    {
        return Err(format!("write outside the transport {stray:x?}"));
    }
    Ok(())
}

fn expect_host(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_return(observed, host(words[0]) as u32)?;
    check_host_map(observed)
}

fn expect_read_mask(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_return(observed, read_mask(words[0]))
}

fn expect_read(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, sample] = *words else {
        return Err("read words".into());
    };
    check_return(observed, sample)?;
    check_host_map(observed)?;
    if !observed
        .effects
        .contains(&PhyEffect::Write(I2C_READ_MASK, !read_mask(block)))
    {
        return Err("no complemented read mask".into());
    }
    check_command(observed, block, selector(block, register) | COMMAND_READ)
}

fn expect_write(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, data, _] = *words else {
        return Err("write words".into());
    };
    if observed
        .effects
        .iter()
        .any(|e| matches!(e, PhyEffect::Write(address, _) if *address == I2C_READ_MASK))
    {
        return Err("a write selects a read mask".into());
    }
    check_command(
        observed,
        block,
        selector(block, register) | data << DATA_SHIFT | COMMAND_WRITE,
    )
}

fn expect_read_field(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [_, _, _, most, least, sample] = *words else {
        return Err("field read words".into());
    };
    check_return(observed, sample >> least & field_mask(most, least))
}

fn expect_write_field(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, most, least, data, sample] = *words else {
        return Err("field write words".into());
    };
    let mask = field_mask(most, least) << least;
    let byte = (sample & !mask | data << least) & 0xff;
    check_command(
        observed,
        block,
        selector(block, register) | byte << DATA_SHIFT | COMMAND_WRITE,
    )
}

const LEAVES: &[Leaf] = &[
    expected(
        leaf(
            "phy_get_i2c_hostid_",
            "open_phy_i2c_trace_phy_get_i2c_hostid_",
            &[("block", Domain::Words(BLOCKS))],
            true,
        ),
        expect_host,
    ),
    expected(
        leaf(
            "phy_get_i2c_read_mask_",
            "open_phy_i2c_trace_phy_get_i2c_read_mask_",
            &[("block", Domain::Words(&BLOCK_BYTES))],
            true,
        ),
        expect_read_mask,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_chip_i2c_readReg",
                        "open_phy_i2c_trace_phy_chip_i2c_readReg",
                        &[
                            ("block", Domain::Words(BLOCKS)),
                            ("host_id", Domain::Words(HOSTS)),
                            ("reg_add", Domain::Words(REGISTERS)),
                        ],
                        true,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_read,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_chip_i2c_writeReg",
                        "open_phy_i2c_trace_phy_chip_i2c_writeReg",
                        &[
                            ("block", Domain::Words(BLOCKS)),
                            ("host_id", Domain::Words(HOSTS)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("data", Domain::Words(BYTES)),
                        ],
                        false,
                    ),
                    written_abi,
                ),
                PORT_STATES,
            ),
            polling,
        ),
        expect_write,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_readReg_Mask",
                        "open_phy_i2c_trace_phy_i2c_readReg_Mask",
                        &[
                            ("block", Domain::Words(FIELD_BLOCKS)),
                            ("host_id", Domain::Words(HOST)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("msb", Domain::Words(MOST_SIGNIFICANT)),
                            ("lsb", Domain::Words(LEAST_SIGNIFICANT)),
                        ],
                        true,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_read_field,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_writeReg_Mask",
                        "open_phy_i2c_trace_phy_i2c_writeReg_Mask",
                        &[
                            ("block", Domain::Words(FIELD_BLOCKS)),
                            ("host_id", Domain::Words(HOST)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("msb", Domain::Words(MOST_SIGNIFICANT)),
                            ("lsb", Domain::Words(LEAST_SIGNIFICANT)),
                            ("data", Domain::Words(FIELD_VALUES)),
                        ],
                        false,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_write_field,
    ),
];

/// Names no pinned input defines: ESP-IDF's `rtc_clk_xtal_freq_get` and the
/// ROM linker script's `_rom_eco_version`, which only `phy_init.o`'s
/// `phy_get_xtal_freq` and `phy_get_rom_ver` in the link closure reference.
/// They resolve to an unmapped address, so reaching one stops the case.
const ABSENT: &[&str] = &["rtc_clk_xtal_freq_get", "_rom_eco_version"];

/// The analog-register I2C transport suite over `libphy.a`.
pub const PHY_I2C: Suite = Suite {
    title: "ESP32-C5 PHY I2C transport leaf comparison",
    id: "phy-i2c",
    archives: &[LIBRARY],
    rom: ROM,
    firmware: None,
    leaves: LEAVES,
    absent: ABSENT,
    roots: &[],
    prepare: None,
    claims: &[],
};

#[cfg(test)]
mod tests {
    use super::*;
    use oer_vendor_scenario_engine::phy::layout::COMMAND_BUSY;

    #[test]
    fn the_expectations_follow_the_reviewed_transport_facts() {
        assert_eq!(host(0x66), 1);
        assert_eq!(host(0x61), 0);
        assert_eq!(read_mask(0x61), 0x100);
        assert_eq!(read_mask(0x62), 0);
        assert_eq!(read_mask(0x00), 0);
        assert_eq!(read_mask(0x70), 0);
        assert_eq!(selector(0x62, 0x07), 0x0765);
        assert_eq!(field_mask(7, 0), 0xff);
        assert_eq!(field_mask(4, 3), 0x3);
        // A busy command is never the completed command either side reads.
        assert_eq!(COMMAND_READ & COMMAND_BUSY, 0);
    }
}
