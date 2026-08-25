//! Closed IEEE 802.15.4 timing transition after common PHY initialization.
//!
//! The pinned public ESP32-S31 path executes the complete common
//! `bt_bb_v2_init_cmplx(1)` body, overrides the shared auxiliary transmit-on
//! delay with argument 50, and then writes receive-on delay 50. This module
//! keeps those three edges inseparable and publishes readiness only after one
//! device-ordering fence.

#![deny(unsafe_code)]

use super::{Ieee802154TaskRegisters, device_fence, generated};

const IEEE802154_DELAY_ARGUMENT: u16 = 50;
const SHARED_DELAY_FIELD_SHIFT: u32 = 16;
const SHARED_DELAY_PRESERVE_MASK: u32 = 0xf800_ffff;
const RX_ON_DELAY_PRESERVE_MASK: u32 = 0xffff_f800;

const fn encode_shared_delay_field(argument: u16) -> u32 {
    (argument.wrapping_sub(10) as u32) << 3
}

const SHARED_DELAY_FIELD_IMAGE: u32 = encode_shared_delay_field(IEEE802154_DELAY_ARGUMENT);
const SHARED_DELAY_REGISTER_INPUT: u32 = SHARED_DELAY_FIELD_IMAGE << SHARED_DELAY_FIELD_SHIFT;
const RX_ON_DELAY_REGISTER_INPUT: u32 = IEEE802154_DELAY_ARGUMENT as u32;

const _: () = assert!(SHARED_DELAY_FIELD_IMAGE == 0x140);
const _: () = assert!(SHARED_DELAY_REGISTER_INPUT == 0x0140_0000);
const _: () = assert!(SHARED_DELAY_PRESERVE_MASK | 0x07ff_0000 == u32::MAX);
const _: () = assert!(SHARED_DELAY_PRESERVE_MASK & 0x07ff_0000 == 0);
const _: () = assert!(RX_ON_DELAY_PRESERVE_MASK | 0x0000_07ff == u32::MAX);
const _: () = assert!(RX_ON_DELAY_PRESERVE_MASK & 0x0000_07ff == 0);
const _: () = assert!(
    generated::Ieee802154SharedTxOnDelayOverride::Delay50.bits() == SHARED_DELAY_REGISTER_INPUT
);
const _: () = assert!(generated::Ieee802154RxOnDelay::Delay50.bits() == RX_ON_DELAY_REGISTER_INPUT);

/// Evidence that the common BTBB body and both IEEE 802.15.4 timing
/// overrides completed in the pinned order.
///
/// Construction is private. Higher layers can receive this marker only from
/// [`Ieee802154TaskRegisters::initialize_baseband_and_ieee802154_timing`].
#[must_use = "IEEE 802.15.4 timing readiness must be retained by the initialized radio state"]
pub struct Ieee802154TimingReady {
    _private: (),
}

trait Ieee802154TimingTransitionPort {
    fn initialize_baseband_v2_arg_one_body_without_fence(&mut self, gain_parameter: u8);
    fn override_shared_tx_on_delay(&mut self, value: generated::Ieee802154SharedTxOnDelayOverride);
    fn set_rx_on_delay(&mut self, value: generated::Ieee802154RxOnDelay);
    fn order_device_accesses(&mut self);
}

fn execute_timing_transition<Port>(port: &mut Port, gain_parameter: u8)
where
    Port: Ieee802154TimingTransitionPort,
{
    port.initialize_baseband_v2_arg_one_body_without_fence(gain_parameter);
    port.override_shared_tx_on_delay(generated::Ieee802154SharedTxOnDelayOverride::Delay50);
    port.set_rx_on_delay(generated::Ieee802154RxOnDelay::Delay50);
    port.order_device_accesses();
}

struct ProductionTimingPort<'a> {
    registers: &'a mut Ieee802154TaskRegisters,
}

impl Ieee802154TimingTransitionPort for ProductionTimingPort<'_> {
    #[allow(
        unsafe_code,
        reason = "the enclosing public transition carries the common-PHY prerequisite"
    )]
    fn initialize_baseband_v2_arg_one_body_without_fence(&mut self, gain_parameter: u8) {
        // SAFETY: this body-only edge is crate-private and
        // `ProductionTimingPort` is constructed only inside
        // `initialize_baseband_and_ieee802154_timing`, whose contract requires
        // the same clocks, reset, common-PHY and gain provenance. The caller
        // immediately applies both timing overrides and the sole final fence.
        unsafe {
            self.registers
                .initialize_baseband_v2_arg_one_body_without_fence(gain_parameter);
        }
    }

    fn override_shared_tx_on_delay(&mut self, value: generated::Ieee802154SharedTxOnDelayOverride) {
        generated::override_ieee802154_shared_tx_on_delay(
            &self
                .registers
                .peripherals
                .btbb
                .shared_radio
                .shared_baseband_tx_timing,
            value,
        );
    }

    fn set_rx_on_delay(&mut self, value: generated::Ieee802154RxOnDelay) {
        generated::set_ieee802154_rx_on_delay(
            self.registers.peripherals.ieee802154_mac.registers(),
            value,
        );
    }

    fn order_device_accesses(&mut self) {
        device_fence();
    }
}

impl Ieee802154TaskRegisters {
    /// Initialize common BTBB state and apply both IEEE 802.15.4 timing
    /// overrides as one closed transition.
    ///
    /// The exact MMIO order is common `bt_bb_v2_init_cmplx(1)`, shared
    /// `AUXILIARY_TX_ON_DELAY=((50-10)<<3)`, MAC `RXON_DELAY=50`, then one
    /// device fence. There is no API that accepts a caller-provided timing
    /// image or exposes the intermediate common-BTBB state.
    ///
    /// # Safety
    ///
    /// The caller must prove that controller clocks/resets are active, common
    /// PHY initialization completed for this same hardware owner, and
    /// `gain_parameter` came from that terminal PHY state. Every physical
    /// owner must remain retained until a verified last-owner PHY teardown;
    /// the task partition must not be reunited into cold ownership after this
    /// transaction without that teardown.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature encodes the cross-crate common-PHY hardware prerequisite"
    )]
    #[doc(hidden)]
    pub unsafe fn initialize_baseband_and_ieee802154_timing(
        &mut self,
        gain_parameter: u8,
    ) -> Ieee802154TimingReady {
        let mut port = ProductionTimingPort { registers: self };
        execute_timing_transition(&mut port, gain_parameter);
        Ieee802154TimingReady { _private: () }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IEEE802154_DELAY_ARGUMENT, Ieee802154TaskRegisters, Ieee802154TimingReady,
        Ieee802154TimingTransitionPort, RX_ON_DELAY_PRESERVE_MASK, SHARED_DELAY_FIELD_IMAGE,
        SHARED_DELAY_PRESERVE_MASK, SHARED_DELAY_REGISTER_INPUT, encode_shared_delay_field,
        execute_timing_transition, generated,
    };
    use std::vec::Vec;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Step {
        BasebandV2BodyWithoutFence(u8),
        SharedDelay { input: u32, result: u32 },
        RxOnDelay { input: u32, result: u32 },
        DeviceFence,
    }

    struct TracePort {
        shared_before: u32,
        rx_on_before: u32,
        steps: Vec<Step>,
    }

    impl Ieee802154TimingTransitionPort for TracePort {
        fn initialize_baseband_v2_arg_one_body_without_fence(&mut self, gain_parameter: u8) {
            self.steps
                .push(Step::BasebandV2BodyWithoutFence(gain_parameter));
        }

        fn override_shared_tx_on_delay(
            &mut self,
            value: generated::Ieee802154SharedTxOnDelayOverride,
        ) {
            let input = value.bits();
            let result = (self.shared_before & SHARED_DELAY_PRESERVE_MASK) | input;
            self.steps.push(Step::SharedDelay { input, result });
        }

        fn set_rx_on_delay(&mut self, value: generated::Ieee802154RxOnDelay) {
            let input = value.bits();
            let result = (self.rx_on_before & RX_ON_DELAY_PRESERVE_MASK) | input;
            self.steps.push(Step::RxOnDelay { input, result });
        }

        fn order_device_accesses(&mut self) {
            self.steps.push(Step::DeviceFence);
        }
    }

    #[test]
    fn public_argument_encodes_the_exact_shared_field_image() {
        assert_eq!(IEEE802154_DELAY_ARGUMENT, 50);
        assert_eq!(encode_shared_delay_field(50), 0x140);
        assert_eq!(SHARED_DELAY_FIELD_IMAGE, 0x140);
        assert_eq!(SHARED_DELAY_REGISTER_INPUT, 0x0140_0000);
    }

    #[test]
    fn transition_has_one_closed_order_and_preserves_unowned_bits() {
        let shared_before = 0xa5a5_5a5a;
        let rx_on_before = 0xdead_beef;
        let mut port = TracePort {
            shared_before,
            rx_on_before,
            steps: Vec::new(),
        };

        execute_timing_transition(&mut port, 0x6d);

        assert_eq!(
            port.steps,
            [
                Step::BasebandV2BodyWithoutFence(0x6d),
                Step::SharedDelay {
                    input: 0x0140_0000,
                    result: (shared_before & 0xf800_ffff) | 0x0140_0000,
                },
                Step::RxOnDelay {
                    input: 50,
                    result: (rx_on_before & 0xffff_f800) | 50,
                },
                Step::DeviceFence,
            ]
        );
    }

    #[test]
    fn unified_unsafe_transition_is_the_only_exported_method_shape() {
        let _: unsafe fn(&mut Ieee802154TaskRegisters, u8) -> Ieee802154TimingReady =
            Ieee802154TaskRegisters::initialize_baseband_and_ieee802154_timing;
    }
}
