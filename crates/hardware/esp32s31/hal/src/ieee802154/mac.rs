//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The task owner exposes only the named single transactions the command
//! executor sequences; the interrupt owners expose the reviewed activation,
//! sample/acknowledge and teardown transitions. None of these types can be
//! constructed outside the HAL: they are transferred only together with the
//! whole-radio route that proves exclusive ownership. Register-level value
//! types are re-exported here so drivers never name the restricted PAC.

use oer_esp32s31_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSetup as PacInterruptSetup, Ieee802154TaskRegisters as PacTaskRegisters,
};

pub use oer_esp32s31_pac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdDurationUnits, Ieee802154Event,
    Ieee802154EventMask, Ieee802154EventObservationError, Ieee802154FrequencyCode,
    Ieee802154InterruptSnapshot, Ieee802154MacCommand, Ieee802154MacControl,
    Ieee802154MacPolicySnapshot, Ieee802154MultipanEnableState, Ieee802154PanIdentity,
    Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154Timer0ThresholdWord,
    Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
};

/// Exclusive task-side owner of the IEEE 802.15.4 MAC registers.
#[must_use = "the IEEE 802.15.4 task owner must be returned to its route"]
pub struct Ieee802154TaskOwner {
    registers: PacTaskRegisters,
}

impl Ieee802154TaskOwner {
    /// Replace the channel frequency code.
    pub fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.registers
            .ieee802154_register_lease()
            .set_frequency_code(code);
    }

    /// Replace the CCA mode.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_mode(mode);
    }

    /// Replace the raw CCA threshold code.
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_threshold_code(threshold);
    }

    /// Replace the MAC control policy fields.
    pub fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.registers
            .ieee802154_register_lease()
            .set_mac_control(control);
    }

    /// Replace the acknowledgement timeout field.
    pub fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.registers
            .ieee802154_register_lease()
            .set_ack_timeout(timeout);
    }

    /// Replace the primary PAN identity.
    pub fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.registers
            .ieee802154_register_lease()
            .set_primary_pan_identity(identity);
    }

    /// Sample the complete static MAC policy image.
    pub fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        self.registers
            .ieee802154_register_lease()
            .mac_policy_snapshot()
    }

    /// Publish the transmit DMA descriptor address.
    pub fn publish_transmit_dma_address(&mut self, address: u32) {
        self.registers
            .ieee802154_register_lease()
            .publish_transmit_dma_address(address);
    }

    /// Publish the receive DMA descriptor address.
    pub fn publish_receive_dma_address(&mut self, address: u32) {
        self.registers
            .ieee802154_register_lease()
            .publish_receive_dma_address(address);
    }

    /// Replace the energy-detection duration.
    pub fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
        self.registers
            .ieee802154_register_lease()
            .set_ed_duration(duration);
    }

    /// Request exactly one MAC command.
    pub fn request_mac_command(&mut self, command: Ieee802154MacCommand) {
        self.registers
            .ieee802154_register_lease()
            .request_mac_command(command);
    }

    /// Enable the acknowledgement-watchdog TIMER0 event.
    pub fn enable_acknowledgement_watchdog_event(&mut self) {
        self.registers
            .ieee802154_register_lease()
            .timer_lease()
            .enable_acknowledgement_watchdog_event();
    }

    /// Program and start the acknowledgement watchdog.
    pub fn start_acknowledgement_watchdog(&mut self, threshold: Ieee802154Timer0ThresholdWord) {
        self.registers
            .ieee802154_register_lease()
            .timer_lease()
            .start_acknowledgement_watchdog(threshold);
    }

    /// Stop the acknowledgement watchdog.
    pub fn disarm_acknowledgement_watchdog(&mut self) {
        self.registers
            .ieee802154_register_lease()
            .timer_lease()
            .disarm_acknowledgement_watchdog();
    }

    /// Order all preceding device accesses before later ones.
    pub fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its route"]
pub struct Ieee802154InterruptSetupOwner {
    registers: PacInterruptSetup,
}

impl Ieee802154InterruptSetupOwner {
    /// Install the reviewed event/abort baseline, acknowledge one stale
    /// snapshot and return the active interrupt owner. The platform CPU route
    /// must be enabled only afterwards.
    pub fn activate(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptOwner {
        Ieee802154InterruptOwner {
            registers: self.registers.activate(&mut task.registers),
        }
    }
}

/// Active IEEE 802.15.4 hard-IRQ owner.
#[must_use = "the active IEEE 802.15.4 interrupt owner must be deactivated"]
pub struct Ieee802154InterruptOwner {
    registers: PacInterruptRegisters,
}

impl Ieee802154InterruptOwner {
    /// Capture one ISR event/status batch before acknowledging any event.
    pub fn sample_interrupt(&self) -> Ieee802154InterruptSnapshot {
        self.registers.sample_interrupt()
    }

    /// Acknowledge exactly one sampled W1C event image.
    pub fn acknowledge_interrupt(&mut self, snapshot: Ieee802154InterruptSnapshot) {
        self.registers.acknowledge_interrupt(snapshot);
    }

    /// Close one hard-IRQ epoch after the CPU route was disabled and return
    /// inactive setup ownership.
    pub fn deactivate(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptSetupOwner {
        Ieee802154InterruptSetupOwner {
            registers: self.registers.deactivate(&mut task.registers),
        }
    }
}
