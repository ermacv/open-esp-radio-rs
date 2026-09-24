//! ABI and completion-edge delivery for shipping I2C transactions.
use oer_esp32s31_phy::calibration::cold::{
    PhyColdI2cAction, PhyColdI2cOutcome, PhyColdI2cRequest, PhyColdI2cTransaction,
};

/// Isolated entry with explicit ABI words, zero saved registers and zero unused temporaries.
///
/// # Safety
/// Only enter as a guest root, since saved registers are overwritten. `entry`
/// must be executable; all eight words must be readable and valid for that entry.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn open_phy_trace_i2c_entry(entry: u32, words: &[u32; 8]) {
    core::arch::naked_asm!(
        "mv t0, a0",
        "mv t1, a1",
        "lw a0, 0(t1)",
        "lw a1, 4(t1)",
        "lw a2, 8(t1)",
        "lw a3, 12(t1)",
        "lw a4, 16(t1)",
        "lw a5, 20(t1)",
        "lw a6, 24(t1)",
        "lw a7, 28(t1)",
        "li t2, 0",
        "li t3, 0",
        "li t4, 0",
        "li t5, 0",
        "li t6, 0",
        "li s0, 0",
        "li s1, 0",
        "li s2, 0",
        "li s3, 0",
        "li s4, 0",
        "li s5, 0",
        "li s6, 0",
        "li s7, 0",
        "li s8, 0",
        "li s9, 0",
        "li s10, 0",
        "li s11, 0",
        "jr t0",
    );
}

/// Invoke two captured void entries in caller-selected order with zero ABI words.
/// No child result or algorithm is synthesized; both bodies execute normally.
///
/// # Safety
/// Both addresses must identify executable void functions accepting zero input
/// words. The caller supplies a valid stack and any memory those functions need.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn open_phy_trace_two_void_entries(first: u32, second: u32) {
    core::arch::naked_asm!(
        "addi sp, sp, -16",
        "sw ra, 12(sp)",
        "sw a1, 8(sp)",
        "mv t0, a0",
        "li a0, 0",
        "li a1, 0",
        "li a2, 0",
        "li a3, 0",
        "li a4, 0",
        "li a5, 0",
        "li a6, 0",
        "li a7, 0",
        "jalr t0",
        "lw t0, 8(sp)",
        "lw ra, 12(sp)",
        "addi sp, sp, 16",
        "li a0, 0",
        "li a1, 0",
        "li a2, 0",
        "li a3, 0",
        "li a4, 0",
        "li a5, 0",
        "li a6, 0",
        "li a7, 0",
        "jr t0",
    );
}

/// Supply a finite sequence of completion edges to the production transaction.
/// Profiles 0..3 select ADC-rate byte read/write/field read/write; 4..7 select
/// the TX-capacitor high field. Encodings and masked updates remain production code.
/// Every start or delivered edge consumes one harness action. Return 0x10000
/// denotes start failure, 0x10001 an exhausted action schedule, 0x10002 an invalid
/// transition and 0x10003 an unknown profile. This scheduling is not hardware time.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_i2c_transfer(
    profile: u32,
    value: u32,
    maximum_actions: u32,
) -> u32 {
    use oer_esp32s31_hal::phy::i2c::analog_registers::{ADC_RATE_CONFIGURATION, TX_CAPACITOR_HIGH};
    let field = match profile {
        0..=3 => ADC_RATE_CONFIGURATION,
        4..=7 => TX_CAPACITOR_HIGH,
        _ => return 0x10003,
    };
    let request = match profile % 4 {
        0 => PhyColdI2cRequest::read_byte(field.address()),
        1 => PhyColdI2cRequest::write_byte(field.address(), value as u8),
        2 => PhyColdI2cRequest::read_field(field),
        _ => PhyColdI2cRequest::write_field(field, value as u8),
    };
    let (mut registers, _interrupts) = oer_esp32s31_pac::RadioHardware::for_validation()
        .into_wifi()
        .into_running();
    let mut transaction = PhyColdI2cTransaction::new(request);
    for _ in 0..maximum_actions {
        match transaction.action() {
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                if transaction.start_target(registers.radio_phy_mut()).is_err() {
                    return 0x10000;
                }
            }
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                if transaction
                    .observe_target_edge(registers.radio_phy())
                    .is_err()
                {
                    return 0x10002;
                }
            }
            PhyColdI2cAction::Complete(_) => break,
        }
    }
    match transaction.action() {
        PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read { value, .. }) => u32::from(value),
        PhyColdI2cAction::Complete(PhyColdI2cOutcome::Written { .. }) => 0,
        _ => 0x10001,
    }
}

/// Lower the shipping host-selection result to the captured integer ABI.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_i2c_host(block: u32) -> u32 {
    let (mut registers, _interrupts) = oer_esp32s31_pac::RadioHardware::for_validation()
        .into_wifi()
        .into_running();
    oer_esp32s31_phy::validation::configure_and_select_phy_i2c_host(
        registers.radio_phy_mut(),
        block as u8,
    )
}

/// Execute the complete production reset helper, including its bounded poll policy.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_i2c_reset() -> u32 {
    let (mut registers, _interrupts) = oer_esp32s31_pac::RadioHardware::for_validation()
        .into_wifi()
        .into_running();
    match oer_esp32s31_phy::validation::reset_i2c_master(registers.radio_phy_mut()) {
        Ok(()) => 0,
        Err(oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut) => 0x10001,
        Err(_) => 0x10002,
    }
}

pub fn retain() {
    core::hint::black_box(open_phy_trace_two_void_entries as *const ());
    core::hint::black_box(open_phy_trace_i2c_entry as *const ());
    core::hint::black_box(open_phy_trace_i2c_transfer as *const ());
    core::hint::black_box(open_phy_trace_i2c_host as *const ());
    core::hint::black_box(open_phy_trace_i2c_reset as *const ());
}
