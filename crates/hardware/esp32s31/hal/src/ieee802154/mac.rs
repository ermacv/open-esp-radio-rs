//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The task owner exposes the semantic operations the command executor
//! sequences: the static-policy refresh shares its write order and readback
//! with the cold policy transition, so the HAL owns one policy path. The
//! interrupt owners expose the reviewed activation,
//! sample/acknowledge and teardown transitions. None of these types can be
//! constructed outside the HAL: `Ieee802154MacPolicyConfigured::into_operational`
//! hands them out and `Ieee802154OperationalRoute::into_policy_configured`
//! takes both back, so they never exist apart from their exclusive route. Commands and policy use
//! the HAL's semantic vocabulary; only the interrupt event vocabulary, which
//! has no HAL counterpart, is re-exported here.

use oer_esp32s31_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSetup as PacInterruptSetup, Ieee802154MacCommand as PacMacCommand,
    Ieee802154RegisterLease as PacRegisterLease, Ieee802154TaskRegisters as PacTaskRegisters,
    Ieee802154Timer0ThresholdWord as PacTimer0ThresholdWord,
};

pub use oer_esp32s31_pac::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154EventObservationError, Ieee802154InterruptSnapshot, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReason,
    Ieee802154TxAbortReasonObservation,
};

use crate::phy::restore::PhyRouteState;

use crate::ieee802154::{
    backend::ed_duration_units,
    lifecycle::{Ieee802154Channel, Ieee802154ReadbackError},
    policy::{
        Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicy,
        Ieee802154MacPolicyCheckpoint, Ieee802154MacPolicyRefreshBackend,
        Ieee802154MacPolicySnapshot, Ieee802154MacPolicyWrites, Ieee802154PanIdentity,
        refresh_ieee802154_mac_policy,
    },
};

/// One MAC command issued after its policy and DMA publication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154Command {
    /// Start reception into the published RX DMA address.
    Receive,
    /// Start transmission from the published TX DMA address.
    Transmit,
    /// Perform CCA and transmit when the channel is clear.
    ClearChannelThenTransmit,
    /// Start one configured energy-detection/CCA sampling transaction.
    EnergyDetection,
}

impl Ieee802154Command {
    const fn into_pac(self) -> PacMacCommand {
        match self {
            Self::Receive => PacMacCommand::Receive,
            Self::Transmit => PacMacCommand::Transmit,
            Self::ClearChannelThenTransmit => PacMacCommand::ClearChannelThenTransmit,
            Self::EnergyDetection => PacMacCommand::EnergyDetection,
        }
    }
}

/// Exclusive task-side owner of the IEEE 802.15.4 MAC registers.
///
/// Every operation is expressed in the HAL's semantic vocabulary; drivers
/// never name a register-level value type.
#[must_use = "the IEEE 802.15.4 task owner must be returned to its route"]
pub struct Ieee802154TaskOwner {
    registers: PacTaskRegisters,
    /// The route PHY state travels with the registers that contain the PHY.
    phy_state: PhyRouteState,
}

impl Ieee802154TaskOwner {
    pub(crate) const fn new(registers: PacTaskRegisters, phy_state: PhyRouteState) -> Self {
        Self {
            registers,
            phy_state,
        }
    }

    pub(crate) fn into_parts(self) -> (PacTaskRegisters, PhyRouteState) {
        (self.registers, self.phy_state)
    }

    /// Borrow the PAC task lease for one low-level accessor.
    pub(crate) fn lease(&mut self) -> PacRegisterLease<'_> {
        self.registers.ieee802154_register_lease()
    }

    /// Republish the complete static policy, fence it and prove the sampled
    /// policy fields before one command epoch.
    ///
    /// # Errors
    ///
    /// Returns the first policy field whose readback does not match.
    pub fn refresh_mac_policy(
        &mut self,
        policy: Ieee802154MacPolicy,
    ) -> Result<(), Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>> {
        refresh_ieee802154_mac_policy(self, policy)
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

    /// Replace the energy-detection duration; every `u16` fits the field.
    pub fn set_ed_duration(&mut self, units: u16) {
        self.registers
            .ieee802154_register_lease()
            .set_ed_duration(ed_duration_units(units));
    }

    /// Request exactly one MAC command.
    pub fn request_command(&mut self, command: Ieee802154Command) {
        self.registers
            .ieee802154_register_lease()
            .request_mac_command(command.into_pac());
    }

    /// Enable the acknowledgement-watchdog TIMER0 event.
    pub fn enable_acknowledgement_watchdog_event(&mut self) {
        self.registers
            .ieee802154_register_lease()
            .timer_lease()
            .enable_acknowledgement_watchdog_event();
    }

    /// Program the remaining watchdog interval in microseconds and start it.
    pub fn start_acknowledgement_watchdog(&mut self, remaining_microseconds: u32) {
        self.registers
            .ieee802154_register_lease()
            .timer_lease()
            .start_acknowledgement_watchdog(PacTimer0ThresholdWord::new(remaining_microseconds));
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

impl Ieee802154MacPolicyWrites for Ieee802154TaskOwner {
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.registers
            .ieee802154_register_lease()
            .set_frequency_code(channel.frequency_code());
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_mode(mode.into_pac());
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.registers
            .ieee802154_register_lease()
            .set_mac_control(control.into_pac());
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.registers
            .ieee802154_register_lease()
            .set_ack_timeout(timeout.into_pac());
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.registers
            .ieee802154_register_lease()
            .set_primary_pan_identity(identity.into_pac());
    }

    fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }
}

impl Ieee802154MacPolicyRefreshBackend for Ieee802154TaskOwner {
    fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        Ieee802154MacPolicySnapshot::from_pac(
            self.registers
                .ieee802154_register_lease()
                .mac_policy_snapshot(),
        )
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its route"]
pub struct Ieee802154InterruptSetupOwner {
    registers: PacInterruptSetup,
}

impl Ieee802154InterruptSetupOwner {
    pub(crate) const fn new(registers: PacInterruptSetup) -> Self {
        Self { registers }
    }

    pub(crate) fn into_pac(self) -> PacInterruptSetup {
        self.registers
    }

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
    pub(crate) const fn registers(&self) -> &PacInterruptRegisters {
        &self.registers
    }

    pub(crate) fn registers_mut(&mut self) -> &mut PacInterruptRegisters {
        &mut self.registers
    }

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
