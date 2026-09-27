#![no_std]

//! Link-time probes for compiled vendor/Rust analog I2C comparison.
//!
//! Every probe takes the vendor function's integer arguments in its order and
//! drives the shipping ESP32-C5 analog I2C owners. The vendor leaves ignore
//! their `host_id` argument and derive the host from the block; so does the
//! production owner. These wrappers are test-harness artifacts, never driver
//! entry points.

use oer_esp32c5_hal::analog::AnalogI2c;
use oer_esp32c5_pac::{PhyI2cAddress, PhyI2cBlock, PhyI2cHost, RadioPartitions};
use oer_radio_analog::{AnalogField, AnalogRegisterBus, FieldRead, FieldWrite, Step};

/// Bus actions (one start or completion observation each) a probe performs
/// before it reports an exhausted schedule. This bounds the harness only; it
/// is not hardware time.
const MAXIMUM_ACTIONS: u32 = 64;
/// Returned when the block is outside the libphy tables.
const INVALID_ARGUMENT: u32 = 0x1_0000;
/// Returned when the schedule is exhausted before completion.
const EXHAUSTED: u32 = 0x1_0001;

/// Borrow the isolated analog register bus.
fn with_bus<R>(call: impl FnOnce(&mut AnalogI2c) -> R) -> R {
    let RadioPartitions { phy_i2c, .. } = RadioPartitions::for_validation();
    call(&mut AnalogI2c::new(phy_i2c))
}

fn address(block: u32, register: u32) -> Option<PhyI2cAddress> {
    let block = PhyI2cBlock::from_vendor_abi(u8::try_from(block).ok()?)?;
    Some(PhyI2cAddress::new(block, register as u8))
}

fn drive<T>(mut poll: impl FnMut() -> Step<T>) -> Option<T> {
    for _ in 0..MAXIMUM_ACTIONS {
        if let Step::Ready(value) = poll() {
            return Some(value);
        }
    }
    None
}

#[panic_handler]
#[allow(
    clippy::disallowed_methods,
    reason = "isolated verification image has no executor; panic is a terminal probe failure"
)]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

oer_probe_macros::probe! {
    /// `phy_get_i2c_hostid_(block)`: rewrite the host map, return the host.
    pub fn open_phy_i2c_trace_phy_get_i2c_hostid_(block: u32) -> u32 {
        let Some(block) = u8::try_from(block).ok().and_then(PhyI2cBlock::from_vendor_abi) else {
            return INVALID_ARGUMENT;
        };
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        match phy_i2c.configure_and_select_host(block) {
            PhyI2cHost::Host0 => 0,
            PhyI2cHost::Host1 => 1,
        }
    }
}

oer_probe_macros::probe! {
    /// `phy_get_i2c_read_mask_(block)`: the one-hot read mask; zero outside
    /// the table, as the vendor leaf returns.
    pub fn open_phy_i2c_trace_phy_get_i2c_read_mask_(block: u32) -> u32 {
        match u8::try_from(block).ok().and_then(PhyI2cBlock::from_vendor_abi) {
            Some(block) => !block.read_mask_complement_low() & 0x00ff_ffff,
            None => 0,
        }
    }
}

oer_probe_macros::probe! {
    /// `phy_chip_i2c_readReg(block, host_id, reg_add)`.
    pub fn open_phy_i2c_trace_phy_chip_i2c_readReg(block: u32, host_id: u32, reg_add: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        with_bus(|bus| {
            let mut started = false;
            let result = drive(|| {
                if !started {
                    started = bus.try_start_read(address).is_ok();
                    return Step::Pending;
                }
                match bus.try_finish_read(address) {
                    Ok(value) => Step::Ready(value),
                    Err(_) => Step::Pending,
                }
            });
            result.map_or(EXHAUSTED, u32::from)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_chip_i2c_writeReg(block, host_id, reg_add, data)`.
    pub fn open_phy_i2c_trace_phy_chip_i2c_writeReg(block: u32, host_id: u32, reg_add: u32, data: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Ok(data) = u8::try_from(data) else { return INVALID_ARGUMENT };
        with_bus(|bus| {
            let mut started = false;
            let result = drive(|| {
                if !started {
                    started = bus.try_start_write(address, data).is_ok();
                    return Step::Pending;
                }
                match bus.try_finish_write(address) {
                    Ok(()) => Step::Ready(()),
                    Err(_) => Step::Pending,
                }
            });
            result.map_or(EXHAUSTED, |()| 0)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_readReg_Mask(block, host_id, reg_add, msb, lsb)` as a polled
    /// field read over the portable analog bus.
    pub fn open_phy_i2c_trace_phy_i2c_readReg_Mask(block: u32, host_id: u32, reg_add: u32, msb: u32, lsb: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Some(field) = AnalogField::new(address, msb as u8, lsb as u8) else {
            return INVALID_ARGUMENT;
        };
        with_bus(|bus| {
            let mut read = FieldRead::new(field);
            let result = drive(|| read.poll(bus));
            result.map_or(EXHAUSTED, u32::from)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_writeReg_Mask(block, host_id, reg_add, msb, lsb, data)` as a
    /// polled field write. A value wider than the field is rejected before any
    /// bus action, unlike the vendor leaf's unmasked OR.
    pub fn open_phy_i2c_trace_phy_i2c_writeReg_Mask(block: u32, host_id: u32, reg_add: u32, msb: u32, lsb: u32, data: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Some(field) = AnalogField::new(address, msb as u8, lsb as u8) else {
            return INVALID_ARGUMENT;
        };
        let Some(mut write) = u8::try_from(data).ok().and_then(|data| FieldWrite::new(field, data).ok()) else {
            return INVALID_ARGUMENT;
        };
        with_bus(|bus| {
            let result = drive(|| write.poll(bus));
            result.map_or(EXHAUSTED, |()| 0)
        })
    }
}

/// Parallel-sequence polls before a probe reports an exhausted schedule:
/// select, 44 pairs of one start and two idle observations, restore.
const MAXIMUM_PARALLEL_ACTIONS: u32 = 4 * 64;

oer_probe_macros::probe! {
    /// `phy_i2c_paral_write(block0, reg0, data0, block1, reg1, data1, flag)`:
    /// both commands published, host 0 then host 1 polled idle. Production
    /// publishes without the start bit, so only `flag == 0` is accepted.
    pub fn open_phy_i2c_trace_phy_i2c_paral_write(block0: u32, reg0: u32, data0: u32, block1: u32, reg1: u32, data1: u32, flag: u32) -> u32 {
        let byte = |value: u32| u8::try_from(value).ok();
        let (Some(b0), Some(r0), Some(d0), Some(b1), Some(r1), Some(d1)) =
            (byte(block0), byte(reg0), byte(data0), byte(block1), byte(reg1), byte(data1))
        else {
            return INVALID_ARGUMENT;
        };
        if flag != 0 {
            return INVALID_ARGUMENT;
        }
        with_bus(|bus| {
            use oer_radio_analog::{ParallelAnalogBus, ParallelHost};
            bus.start_pair(oer_esp32c5_pac::PhyI2cParallelWrite::new((b0, r0, d0), (b1, r1, d1)));
            for host in [ParallelHost::First, ParallelHost::Second] {
                let mut polls = 0;
                while bus.is_busy(host) {
                    polls += 1;
                    if polls == MAXIMUM_ACTIONS {
                        return EXHAUSTED;
                    }
                }
            }
            0
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_init1()` over the `phy_param` image at `parameters`: the
    /// production parallel initialization with the fields the vendor reads.
    ///
    /// The caller supplies a readable 1080-byte parameter image.
    pub fn open_phy_i2c_trace_phy_i2c_init1(parameters: u32) -> u32 {
        let byte = |offset: usize| {
            // SAFETY: the harness supplies a readable parameter image of the
            // vendor `phy_param` size at `parameters`.
            unsafe { core::ptr::read_volatile((parameters as usize + offset) as *const u8) }
        };
        let halfword = |offset: usize| u16::from(byte(offset)) | (u16::from(byte(offset + 1)) << 8);
        let inputs = oer_esp32c5_pac::PhyI2cInitializationInputs {
            parameter_f5: byte(0xf5),
            parameter_f6: byte(0xf6),
            parameter_f7: byte(0xf7),
            parameter_f8: byte(0xf8),
            parameter_f9: byte(0xf9),
            parameter_fa: byte(0xfa),
            parameter_fb: byte(0xfb),
            parameter_fc: byte(0xfc),
            parameter_410: byte(0x410),
            parameter_412: byte(0x412),
            parameter_416: halfword(0x416),
        };
        with_bus(|bus| {
            let mut sequence = oer_esp32c5_hal::analog::initialization(inputs);
            for _ in 0..MAXIMUM_PARALLEL_ACTIONS {
                if sequence.poll(bus) == Step::Ready(()) {
                    return 0;
                }
            }
            EXHAUSTED
        })
    }
}
