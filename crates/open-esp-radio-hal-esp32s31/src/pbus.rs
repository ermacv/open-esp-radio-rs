//! Owned access to the recovered ESP32-S31 PHY PBus registers.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::{
    power::{modem_syscon, phy_pbus},
    Field32, RadioRegisters, Register32,
};

/// A PBus command could not be published or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbusError {
    /// The hardware busy bit was set at the single allowed observation.
    Busy,
    /// The caller supplied a selector outside the recovered four-bit field.
    SelectorOutOfRange,
    /// The caller supplied a path outside the recovered two-bit field.
    PathOutOfRange,
    /// The caller supplied a test value outside the recovered nine-bit field.
    ValueOutOfRange,
}

#[cfg(target_arch = "riscv32")]
fn field_value(field: Field32, value: u32) -> Result<u32, PbusError> {
    field.checked_value(value).ok_or(PbusError::ValueOutOfRange)
}

/// Enter the PBus debug mode used before force-test transactions.
///
/// Basis: complete rev0 ROM `phy_pbus_debugmode` and
/// `phy_pbus_force_mode(1)`. They clear `MODE.WORK_MODE_ENABLE`, then set
/// `COMMAND.DEBUG_MODE_ENABLE`, with one fresh read before each write.
#[cfg(target_arch = "riscv32")]
pub fn configure_debug_mode(registers: &mut RadioRegisters) {
    let work_mode = phy_pbus::mode::WORK_MODE_ENABLE.mask();
    registers.modify32(phy_pbus::MODE, work_mode, 0);

    let debug_mode = phy_pbus::command::DEBUG_MODE_ENABLE.mask();
    registers.modify32(phy_pbus::COMMAND, debug_mode, debug_mode);
}

/// Enter PBus work mode and sample the optional settle-pulse condition once.
///
/// Basis: complete rev0 ROM `phy_pbus_force_mode(0)`. It clears
/// `COMMAND.DEBUG_MODE_ENABLE`, sets `MODE.WORK_MODE_ENABLE`, and samples
/// `MODEM_SYSCON.WIFI_BB_CFG` bit 1. The returned boolean only reports that
/// instruction-evidenced condition; the delayed pulse itself belongs to the
/// caller's async transition.
#[cfg(target_arch = "riscv32")]
pub fn configure_work_mode(registers: &mut RadioRegisters) -> bool {
    let debug_mode = phy_pbus::command::DEBUG_MODE_ENABLE.mask();
    registers.modify32(phy_pbus::COMMAND, debug_mode, 0);

    let work_mode = phy_pbus::mode::WORK_MODE_ENABLE.mask();
    registers.modify32(phy_pbus::MODE, work_mode, work_mode);

    modem_syscon::wifi_bb_cfg::PBUS_WORK_MODE_SETTLE_PULSE_REQUIRED
        .extract(registers.read32(modem_syscon::WIFI_BB_CFG))
        != 0
}

/// Publish one PBus force-test command after one fail-fast busy sample.
///
/// Basis: complete rev0 ROM `phy_pbus_force_test` at `0x2f824228`, size
/// `0x42`. The field encoding and final transaction bit are exact. Rust adds
/// only the pre-publication busy rejection so ownership never overwrites an
/// in-flight command.
#[cfg(target_arch = "riscv32")]
pub fn try_start_force_test(
    registers: &mut RadioRegisters,
    selector: u8,
    path: u8,
    test_value: u16,
) -> Result<(), PbusError> {
    if phy_pbus::status_clock_force::BUSY.extract(registers.read32(phy_pbus::STATUS_CLOCK_FORCE))
        != 0
    {
        return Err(PbusError::Busy);
    }
    if u32::from(selector) > phy_pbus::command::SELECTOR.max_value() {
        return Err(PbusError::SelectorOutOfRange);
    }
    if u32::from(path) > phy_pbus::command::PATH.max_value() {
        return Err(PbusError::PathOutOfRange);
    }
    if u32::from(test_value) > phy_pbus::command::TEST_VALUE.max_value() {
        return Err(PbusError::ValueOutOfRange);
    }

    let command_mask = phy_pbus::command::TRANSACTION_START.mask()
        | phy_pbus::command::SELECTOR.mask()
        | phy_pbus::command::TEST_VALUE.mask()
        | phy_pbus::command::PATH.mask();
    let command = phy_pbus::command::TRANSACTION_START.mask()
        | field_value(phy_pbus::command::SELECTOR, u32::from(selector))?
        | field_value(phy_pbus::command::PATH, u32::from(path))?
        | field_value(phy_pbus::command::TEST_VALUE, u32::from(test_value))?;
    registers.modify32(phy_pbus::COMMAND, command_mask, command);
    Ok(())
}

/// Observe one force-test completion edge and clear its transaction bit.
///
/// Basis: the suffix of complete rev0 ROM `phy_pbus_force_test`. ROM spins
/// until `STATUS_CLOCK_FORCE.BUSY` clears and then clears
/// `COMMAND.TRANSACTION_START`; this HAL performs only one observation so the
/// Rust async owner controls retries and timeout.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_force_test(registers: &mut RadioRegisters) -> Result<(), PbusError> {
    if phy_pbus::status_clock_force::BUSY.extract(registers.read32(phy_pbus::STATUS_CLOCK_FORCE))
        != 0
    {
        return Err(PbusError::Busy);
    }
    let transaction = phy_pbus::command::TRANSACTION_START.mask();
    registers.modify32(phy_pbus::COMMAND, transaction, 0);
    Ok(())
}

/// Read the nine-bit packed result selected by the ROM address/shift tables.
///
/// Basis: complete rev0 ROM `phy_pbus_rd` plus its jump tables at
/// `0x2f84d910` and `0x2f84d924`. Result-window locations are exact; except
/// for selector 1's low RX-DCO consumer, their analog meanings remain
/// intentionally unknown.
#[cfg(target_arch = "riscv32")]
pub fn read_result(registers: &RadioRegisters, selector: u8, path: u8) -> Option<u16> {
    let (register, field): (Register32, Field32) = match selector {
        0 if path == 1 => (
            phy_pbus::READ_RESULT_4,
            phy_pbus::read_result_4::RESULT_WINDOW_2_UNKNOWN,
        ),
        0 => (
            phy_pbus::READ_RESULT_4,
            phy_pbus::read_result_4::RESULT_WINDOW_1_UNKNOWN,
        ),
        1 if path == 1 => (
            phy_pbus::READ_RESULT_0,
            phy_pbus::read_result_0::RESULT_WINDOW_1_UNKNOWN,
        ),
        1 => (
            phy_pbus::READ_RESULT_0,
            phy_pbus::read_result_0::RESULT_WINDOW_0_RX_DCO,
        ),
        2 if path == 1 => (
            phy_pbus::READ_RESULT_1,
            phy_pbus::read_result_1::RESULT_WINDOW_0_UNKNOWN,
        ),
        2 => (
            phy_pbus::READ_RESULT_2,
            phy_pbus::read_result_2::RESULT_WINDOW_2_UNKNOWN,
        ),
        3 if path == 1 => (
            phy_pbus::READ_RESULT_2,
            phy_pbus::read_result_2::RESULT_WINDOW_1_UNKNOWN,
        ),
        3 => (
            phy_pbus::READ_RESULT_2,
            phy_pbus::read_result_2::RESULT_WINDOW_0_UNKNOWN,
        ),
        4 if path == 1 => (
            phy_pbus::READ_RESULT_3,
            phy_pbus::read_result_3::RESULT_WINDOW_0_UNKNOWN,
        ),
        4 => (
            phy_pbus::READ_RESULT_4,
            phy_pbus::read_result_4::RESULT_WINDOW_2_UNKNOWN,
        ),
        5 => (
            phy_pbus::READ_RESULT_4,
            phy_pbus::read_result_4::RESULT_WINDOW_0_UNKNOWN,
        ),
        _ => return None,
    };
    Some(field.extract(registers.read32(register)) as u16)
}

/// Enable or disable both recovered RX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_rxclk_en` at `0x2f827cf6`, size
/// `0x20`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_clock(registers: &mut RadioRegisters, enabled: bool) {
    let field = phy_pbus::status_clock_force::RX_CLOCK_ENABLE_PAIR;
    let value = if enabled { field.mask() } else { 0 };
    registers.modify32(phy_pbus::STATUS_CLOCK_FORCE, field.mask(), value);
}

/// Enable or disable both recovered TX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_txclk_en` at `0x2f827cd2`, size
/// `0x24`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_clock(registers: &mut RadioRegisters, enabled: bool) {
    let field = phy_pbus::status_clock_force::TX_CLOCK_ENABLE_PAIR;
    let value = if enabled { field.mask() } else { 0 };
    registers.modify32(phy_pbus::STATUS_CLOCK_FORCE, field.mask(), value);
}
