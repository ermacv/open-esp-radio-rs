//! Typed lower-level ownership for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! Every MMIO operation in this module is routed through the reviewed
//! generated `IEEE802154_MAC` peripheral. The narrow lease exposes only the
//! first field-sized operations needed by HAL; neither the generated register
//! block nor numeric addresses can escape it.

#![forbid(unsafe_code)]

use super::WifiRadioRegisters;

/// Opaque eight-bit value accepted by the MAC frequency-code register.
///
/// This is deliberately not an IEEE channel number. The checked 2.4 GHz
/// channel mapping is source-confirmed and owned by the HAL; the PAC type
/// still represents the complete recovered field rather than silently
/// narrowing register geometry to one operating mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154FrequencyCode(u8);

impl Ieee802154FrequencyCode {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Return the field value, not a complete register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl From<u8> for Ieee802154FrequencyCode {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

/// One five-bit coexistence priority value.
///
/// The value is intentionally not a complete PTI register image. The PAC
/// lease places it through named generated fields and preserves all unrelated
/// bits.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Pti(u8);

impl Ieee802154Pti {
    pub const MAX: u8 = 0x1f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the bounded field value, not a shifted register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// One source-confirmed clear-channel-assessment mode.
///
/// The discriminants are field values, not shifted register images.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154CcaMode {
    Carrier = 0,
    EnergyDetection = 1,
    CarrierOrEnergyDetection = 2,
    CarrierAndEnergyDetection = 3,
}

impl Ieee802154CcaMode {
    pub const fn field_value(self) -> u8 {
        self as u8
    }

    const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::Carrier,
            1 => Self::EnergyDetection,
            2 => Self::CarrierOrEnergyDetection,
            3 => Self::CarrierAndEnergyDetection,
            _ => unreachable!(),
        }
    }
}

/// Sixteen-bit ACK-timeout field value.
///
/// The PAC deliberately does not assign physical units. The HAL owns the
/// source-confirmed conversion between microseconds and this field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154AckTimeoutUnits(u16);

impl Ieee802154AckTimeoutUnits {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

/// Source-confirmed MAC control fields programmed as one semantic policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacControl {
    tx_auto_ack: bool,
    rx_auto_ack: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    enhanced_pending: bool,
}

impl Ieee802154MacControl {
    pub const fn new(
        tx_auto_ack: bool,
        rx_auto_ack: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        enhanced_pending: bool,
    ) -> Self {
        Self {
            tx_auto_ack,
            rx_auto_ack,
            enhanced_ack_tx,
            coordinator,
            promiscuous,
            enhanced_pending,
        }
    }

    pub const fn tx_auto_ack(self) -> bool {
        self.tx_auto_ack
    }

    pub const fn rx_auto_ack(self) -> bool {
        self.rx_auto_ack
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn enhanced_pending(self) -> bool {
        self.enhanced_pending
    }
}

/// Address-filter identity for the public API's primary PAN context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PanIdentity {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl Ieee802154PanIdentity {
    pub const fn new(pan_id: u16, short_address: u16, extended_address: [u8; 8]) -> Self {
        Self {
            pan_id,
            short_address,
            extended_address,
        }
    }

    pub const fn pan_id(self) -> u16 {
        self.pan_id
    }

    pub const fn short_address(self) -> u16 {
        self.short_address
    }

    pub const fn extended_address(self) -> [u8; 8] {
        self.extended_address
    }
}

/// Opaque three-bit receive-state observation.
///
/// Only the comparison around the publicly identified `RECEIVE_SFD` value is
/// exposed. Zero is intentionally not named `idle` until lifecycle evidence
/// proves that interpretation for the ESP32-S31.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStateCode(u8);

impl Ieee802154RxStateCode {
    pub const MAX: u8 = 0x07;
    pub const RECEIVE_SFD: u8 = 1;

    pub const fn is_receive_sfd(self) -> bool {
        self.0 == Self::RECEIVE_SFD
    }

    pub const fn is_after_receive_sfd(self) -> bool {
        self.0 > Self::RECEIVE_SFD
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    const fn from_field(value: u8) -> Self {
        Self(value)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// Opaque four-bit transmit-state observation.
///
/// No individual value is assigned a lifecycle meaning by this foundation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxStateCode(u8);

impl Ieee802154TxStateCode {
    pub const MAX: u8 = 0x0f;

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    const fn from_field(value: u8) -> Self {
        Self(value)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// One paired receive/transmit state sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154StateSnapshot {
    rx: Ieee802154RxStateCode,
    tx: Ieee802154TxStateCode,
}

/// Raw paired CPU-route observation from the validation-only PAC sidecar.
///
/// This type contains evidence only: it cannot expose a register pointer or
/// perform a route write. Pure decoding and reset predicates belong to the
/// IEEE 802.15.4 IRQ crate above the PAC boundary.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RouteRawReadback {
    core0: u32,
    core1: u32,
}

#[cfg(feature = "validation-probes")]
impl Ieee802154RouteRawReadback {
    /// Return the complete core-zero source-132 route word.
    pub const fn core0_bits(self) -> u32 {
        self.core0
    }

    /// Return the complete core-one source-132 route word.
    pub const fn core1_bits(self) -> u32 {
        self.core1
    }
}

/// Read-back image of the interrupt-masked IEEE 802.15.4 MAC foundation.
///
/// This snapshot deliberately excludes `EVENT_STATUS`: the pinned public LL
/// performs a masked self-write there, but the underlying modified-write
/// semantics are not authoritative yet.  Event clearing belongs to the later
/// IRQ ownership transition, after that gap is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FoundationSnapshot {
    enabled_events: u16,
    enabled_rx_aborts: u32,
    enabled_tx_aborts: u32,
    ed_uses_average: bool,
    txrx_pti: Ieee802154Pti,
    ack_pti: Ieee802154Pti,
}

/// Readback of the static, interrupt-masked MAC policy subset.
///
/// TX power is deliberately absent: its dBm-to-code table remains an opaque
/// RF/BTBB dependency, so this snapshot is not a complete vendor PIB image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicySnapshot {
    frequency_code: Ieee802154FrequencyCode,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ack_timeout: Ieee802154AckTimeoutUnits,
    control: Ieee802154MacControl,
    multipan_enable_mask: u8,
    identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicySnapshot {
    #[doc(hidden)]
    pub const fn new(
        frequency_code: Ieee802154FrequencyCode,
        cca_mode: Ieee802154CcaMode,
        cca_threshold_code: i8,
        ack_timeout: Ieee802154AckTimeoutUnits,
        control: Ieee802154MacControl,
        multipan_enable_mask: u8,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            frequency_code,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            multipan_enable_mask,
            identity,
        }
    }

    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeoutUnits {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_mask(self) -> u8 {
        self.multipan_enable_mask
    }

    pub const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

impl Ieee802154FoundationSnapshot {
    /// Construct a field-level image for a platform-independent read-back.
    ///
    /// The arguments are bounded semantic fields, never shifted or complete
    /// register images. Production snapshots are sampled by the PAC lease;
    /// this constructor also lets the HAL verify its transition against a
    /// host backend without duplicating the register model.
    #[doc(hidden)]
    pub const fn new(
        enabled_events: u16,
        enabled_rx_aborts: u32,
        enabled_tx_aborts: u32,
        ed_uses_average: bool,
        txrx_pti: Ieee802154Pti,
        ack_pti: Ieee802154Pti,
    ) -> Self {
        Self {
            enabled_events,
            enabled_rx_aborts,
            enabled_tx_aborts,
            ed_uses_average,
            txrx_pti,
            ack_pti,
        }
    }

    pub const fn enabled_events(self) -> u16 {
        self.enabled_events
    }

    pub const fn enabled_rx_aborts(self) -> u32 {
        self.enabled_rx_aborts
    }

    pub const fn enabled_tx_aborts(self) -> u32 {
        self.enabled_tx_aborts
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> Ieee802154Pti {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> Ieee802154Pti {
        self.ack_pti
    }
}

impl Ieee802154StateSnapshot {
    pub const fn new(rx: Ieee802154RxStateCode, tx: Ieee802154TxStateCode) -> Self {
        Self { rx, tx }
    }

    pub const fn rx(self) -> Ieee802154RxStateCode {
        self.rx
    }

    pub const fn tx(self) -> Ieee802154TxStateCode {
        self.tx
    }

    /// Test only the observed numeric state codes.
    ///
    /// This is not a reset-readiness or quiescence claim. Those semantic
    /// predicates require a reviewed lifecycle and shared-reset model.
    pub const fn all_codes_zero(self) -> bool {
        self.rx.is_zero() && self.tx.is_zero()
    }
}

/// Narrow borrow reserving the unique radio-register owner for one
/// IEEE 802.15.4 transaction.
///
/// The generated peripheral remains inside [`WifiRadioRegisters`]. Only named
/// field operations are available through this lease, so HAL cannot recover
/// its register block, addresses, or raw images.
#[must_use = "dropping the lease releases the unique radio-register borrow"]
#[doc(hidden)]
pub struct Ieee802154RegisterLease<'registers> {
    registers: &'registers mut WifiRadioRegisters,
}

impl Ieee802154RegisterLease<'_> {
    /// Mask every MAC event while the IRQ ownership split is not active.
    ///
    /// This touches `EVENT_ENABLE`, never the unresolved `EVENT_STATUS`
    /// modified-write register.
    pub fn mask_all_events(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .event_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Mask every receive-abort source before a receive dataplane exists.
    pub fn mask_all_rx_aborts(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .rx_abort_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Mask every transmit-abort source before a transmit dataplane exists.
    pub fn mask_all_tx_aborts(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .tx_abort_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Select the vendor foundation's average energy-detection sampler.
    pub fn select_average_ed_sampling(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| writer.ed_sample_mode().average());
    }

    /// Replace only the generated eight-bit MAC frequency-code field.
    pub fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.registers
            .peripherals
            .ieee802154_mac
            .channel()
            .modify(|_, writer| writer.frequency_code().set(code.value()));
    }

    /// Replace the CCA mode through the generated enumerated field.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| match mode {
                Ieee802154CcaMode::Carrier => writer.cca_mode().carrier(),
                Ieee802154CcaMode::EnergyDetection => writer.cca_mode().energy_detection(),
                Ieee802154CcaMode::CarrierOrEnergyDetection => {
                    writer.cca_mode().carrier_or_energy_detection()
                }
                Ieee802154CcaMode::CarrierAndEnergyDetection => {
                    writer.cca_mode().carrier_and_energy_detection()
                }
            });
    }

    /// Replace the source-defined signed CCA threshold code.
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| writer.cca_threshold_code().set(threshold as u8));
    }

    /// Replace the ACK timeout field without assigning units at the PAC layer.
    pub fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ack_timeout()
            .modify(|_, writer| writer.timeout().set(timeout.value()));
    }

    /// Apply the six public PIB control fields in vendor update order.
    pub fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        let register = self.registers.peripherals.ieee802154_mac.control();
        register.modify(|_, writer| writer.auto_ack_tx().bit(control.tx_auto_ack()));
        register.modify(|_, writer| writer.auto_ack_rx().bit(control.rx_auto_ack()));
        register.modify(|_, writer| writer.enhanced_ack_tx().bit(control.enhanced_ack_tx()));
        register.modify(|_, writer| writer.coordinator().bit(control.coordinator()));
        register.modify(|_, writer| writer.promiscuous().bit(control.promiscuous()));
        register.modify(|_, writer| writer.pending_enhanced().bit(control.enhanced_pending()));
    }

    /// Program the public API's primary PAN identity.
    ///
    /// Each address setter first enables context zero, matching the public LL
    /// and preserving any other enabled contexts.
    pub fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let enable_primary = || {
            mac.control().modify(|reader, writer| {
                writer
                    .multipan_enable_mask()
                    .set(reader.multipan_enable_mask().bits() | 1)
            });
        };

        enable_primary();
        mac.multipan0_pan_id()
            .modify(|_, writer| writer.pan_id().set(identity.pan_id()));
        enable_primary();
        mac.multipan0_short_address()
            .modify(|_, writer| writer.address().set(identity.short_address()));
        enable_primary();
        let address = identity.extended_address();
        mac.multipan0_extended_address_low().modify(|_, writer| {
            writer.address_word().set(u32::from_le_bytes([
                address[0], address[1], address[2], address[3],
            ]))
        });
        mac.multipan0_extended_address_high().modify(|_, writer| {
            writer.address_word().set(u32::from_le_bytes([
                address[4], address[5], address[6], address[7],
            ]))
        });
    }

    /// Replace only the generated five-bit TX/RX coexistence PTI field.
    pub fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.registers
            .peripherals
            .ieee802154_mac
            .coex_pti()
            .modify(|_, writer| writer.txrx_pti().set(pti.value()));
    }

    /// Replace only the generated five-bit ACK coexistence PTI field.
    pub fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.registers
            .peripherals
            .ieee802154_mac
            .coex_pti()
            .modify(|_, writer| writer.ack_pti().set(pti.value()));
    }

    /// Sample only fields written by the interrupt-masked foundation.
    pub fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let event_enable = mac.event_enable().read();
        let rx_abort_enable = mac.rx_abort_enable().read();
        let tx_abort_enable = mac.tx_abort_enable().read();
        let ed_config = mac.ed_config().read();
        let coex_pti = mac.coex_pti().read();

        Ieee802154FoundationSnapshot {
            enabled_events: event_enable.events().bits(),
            enabled_rx_aborts: rx_abort_enable.events().bits(),
            enabled_tx_aborts: tx_abort_enable.events().bits(),
            ed_uses_average: ed_config.ed_sample_mode().is_average(),
            txrx_pti: Ieee802154Pti(coex_pti.txrx_pti().bits()),
            ack_pti: Ieee802154Pti(coex_pti.ack_pti().bits()),
        }
    }

    /// Sample the complete static MAC-policy subset once per backing word.
    pub fn mac_policy_snapshot(&self) -> Ieee802154MacPolicySnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let channel = mac.channel().read();
        let ed_config = mac.ed_config().read();
        let ack_timeout = mac.ack_timeout().read();
        let control = mac.control().read();
        let pan_id = mac.multipan0_pan_id().read();
        let short_address = mac.multipan0_short_address().read();
        let extended_low = mac.multipan0_extended_address_low().read();
        let extended_high = mac.multipan0_extended_address_high().read();
        let low = extended_low.address_word().bits().to_le_bytes();
        let high = extended_high.address_word().bits().to_le_bytes();

        Ieee802154MacPolicySnapshot {
            frequency_code: Ieee802154FrequencyCode(channel.frequency_code().bits()),
            cca_mode: Ieee802154CcaMode::from_field(ed_config.cca_mode().bits()),
            cca_threshold_code: ed_config.cca_threshold_code().bits() as i8,
            ack_timeout: Ieee802154AckTimeoutUnits(ack_timeout.timeout().bits()),
            control: Ieee802154MacControl::new(
                control.auto_ack_tx().bit_is_set(),
                control.auto_ack_rx().bit_is_set(),
                control.enhanced_ack_tx().bit_is_set(),
                control.coordinator().bit_is_set(),
                control.promiscuous().bit_is_set(),
                control.pending_enhanced().bit_is_set(),
            ),
            multipan_enable_mask: control.multipan_enable_mask().bits(),
            identity: Ieee802154PanIdentity::new(
                pan_id.pan_id().bits(),
                short_address.address().bits(),
                [
                    low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
                ],
            ),
        }
    }

    /// Sample the generated receive and transmit state fields once each.
    pub fn state_snapshot(&self) -> Ieee802154StateSnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let rx = Ieee802154RxStateCode::from_field(mac.rx_status().read().state().bits());
        let tx = Ieee802154TxStateCode::from_field(mac.tx_status().read().state().bits());
        Ieee802154StateSnapshot::new(rx, tx)
    }

    /// Observe whether MAC event delivery is still masked for the closed
    /// `EVENT_STATUS` validation transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_enable_events(&self) -> u16 {
        crate::svd::ieee802154_event_status_validation::event_enable_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Enable exactly the two timer events for the closed validation probe.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_timer_events(&mut self) {
        crate::svd::ieee802154_event_status_validation::enable_timer_events(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Mask every event before the validation probe cleans selected status.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_all_events(&mut self) {
        crate::svd::ieee802154_event_status_validation::disable_all_events(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Sample the source-132 interrupt-route words for both CPU cores without
    /// changing either route.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_interrupt_route_readback(&self) -> Ieee802154RouteRawReadback {
        let readback = crate::svd::ieee802154_route_validation::read_route_words(
            &self.registers.peripherals.ieee802154_mac,
        );
        Ieee802154RouteRawReadback {
            core0: readback.core0_bits(),
            core1: readback.core1_bits(),
        }
    }

    /// Sample `EVENT_STATUS` without assigning an acknowledge access class.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_status_events(&self) -> u16 {
        crate::svd::ieee802154_event_status_validation::event_status_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Sample the complete timer-zero counter during the closed validation
    /// transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer0_value(&self) -> u32 {
        crate::svd::ieee802154_event_status_validation::timer0_value(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Sample the complete timer-one counter during the closed validation
    /// transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer1_value(&self) -> u32 {
        crate::svd::ieee802154_event_status_validation::timer1_value(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Program both independent event-status validation timers.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_event_timer_thresholds(&mut self, threshold: u32) {
        crate::svd::ieee802154_event_status_validation::set_timer_thresholds(
            &self.registers.peripherals.ieee802154_mac,
            threshold,
        );
    }

    /// Start validation timer zero without enabling event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::start_timer0(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Stop validation timer zero without changing event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::stop_timer0(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Start validation timer one without enabling event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::start_timer1(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Stop validation timer one without changing event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::stop_timer1(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Write only timer-zero's event bit in the validation-only raw API.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::write_timer0_event(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Write only timer-one's event bit in the validation-only raw API.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::write_timer1_event(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Order memory and device accesses at a descriptor/MMIO boundary.
    pub fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }
}

impl WifiRadioRegisters {
    /// Borrow the reserved IEEE 802.15.4 register capability.
    ///
    /// No generic PAC or register block can be recovered from the result.
    #[doc(hidden)]
    pub fn ieee802154_register_lease(&mut self) -> Ieee802154RegisterLease<'_> {
        Ieee802154RegisterLease { registers: self }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154FoundationSnapshot,
        Ieee802154FrequencyCode, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
        Ieee802154PanIdentity, Ieee802154Pti, Ieee802154RxStateCode, Ieee802154StateSnapshot,
        Ieee802154TxStateCode,
    };
    use crate::RadioHardware;

    #[test]
    fn frequency_code_does_not_claim_an_ieee_channel_mapping() {
        assert_eq!(Ieee802154FrequencyCode::new(0).value(), 0);
        assert_eq!(Ieee802154FrequencyCode::new(u8::MAX).value(), u8::MAX);
    }

    #[test]
    fn foundation_snapshot_exposes_fields_without_complete_register_images() {
        let snapshot = Ieee802154FoundationSnapshot::new(
            0,
            0,
            0,
            true,
            Ieee802154Pti::new(3).expect("five-bit PTI"),
            Ieee802154Pti::new(3).expect("five-bit PTI"),
        );

        assert_eq!(snapshot.enabled_events(), 0);
        assert_eq!(snapshot.enabled_rx_aborts(), 0);
        assert_eq!(snapshot.enabled_tx_aborts(), 0);
        assert!(snapshot.ed_uses_average());
        assert_eq!(snapshot.txrx_pti().value(), 3);
        assert_eq!(snapshot.ack_pti().value(), 3);
    }

    #[test]
    fn mac_policy_snapshot_keeps_typed_fields_and_little_endian_identity() {
        let control = Ieee802154MacControl::new(true, false, true, false, true, false);
        let identity = Ieee802154PanIdentity::new(
            0x1234,
            0xabcd,
            [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        );
        let snapshot = Ieee802154MacPolicySnapshot::new(
            Ieee802154FrequencyCode::new(78),
            Ieee802154CcaMode::CarrierAndEnergyDetection,
            -75,
            Ieee802154AckTimeoutUnits::new(108),
            control,
            0b0101,
            identity,
        );

        assert_eq!(snapshot.frequency_code().value(), 78);
        assert_eq!(
            snapshot.cca_mode(),
            Ieee802154CcaMode::CarrierAndEnergyDetection
        );
        assert_eq!(snapshot.cca_threshold_code(), -75);
        assert_eq!(snapshot.ack_timeout().value(), 108);
        assert_eq!(snapshot.control(), control);
        assert_eq!(snapshot.multipan_enable_mask(), 0b0101);
        assert_eq!(snapshot.identity(), identity);
    }

    #[test]
    fn pti_constructor_never_creates_a_shifted_or_oversized_image() {
        assert_eq!(Ieee802154Pti::new(0).map(Ieee802154Pti::value), Some(0));
        assert_eq!(
            Ieee802154Pti::new(Ieee802154Pti::MAX).map(Ieee802154Pti::value),
            Some(Ieee802154Pti::MAX)
        );
        assert_eq!(Ieee802154Pti::new(Ieee802154Pti::MAX + 1), None);
    }

    #[test]
    fn state_codes_are_bounded_and_expose_only_reviewed_predicates() {
        let zero_rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
        let zero_tx = Ieee802154TxStateCode::for_validation(0).expect("four-bit state");
        let sfd = Ieee802154RxStateCode::for_validation(1).expect("three-bit state");
        let after_sfd = Ieee802154RxStateCode::for_validation(2).expect("three-bit state");

        assert!(Ieee802154StateSnapshot::new(zero_rx, zero_tx).all_codes_zero());
        assert!(sfd.is_receive_sfd());
        assert!(!sfd.is_after_receive_sfd());
        assert!(after_sfd.is_after_receive_sfd());
        assert_eq!(after_sfd.value(), 2);
        assert_eq!(zero_tx.value(), 0);
        assert_eq!(
            Ieee802154RxStateCode::for_validation(Ieee802154RxStateCode::MAX + 1),
            None
        );
        assert_eq!(
            Ieee802154TxStateCode::for_validation(Ieee802154TxStateCode::MAX + 1),
            None
        );
    }

    #[test]
    fn nonzero_state_code_fails_only_the_numeric_zero_predicate() {
        let rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
        let tx = Ieee802154TxStateCode::for_validation(1).expect("four-bit state");
        let snapshot = Ieee802154StateSnapshot::new(rx, tx);

        assert!(!snapshot.all_codes_zero());
        assert_eq!(snapshot.rx().value(), 0);
        assert_eq!(snapshot.tx().value(), 1);
    }

    #[test]
    fn register_lease_borrows_the_existing_unique_radio_owner() {
        let mut cold = RadioHardware::for_validation().into_wifi();
        let mut lease = cold.radio_mut().ieee802154_register_lease();

        // Host execution reaches only the existing architecture-neutral
        // device fence; MMIO operations remain compiled but are not executed.
        lease.order_device_accesses();
    }

    #[test]
    fn generated_mac_geometry_is_owned_by_the_radio_partition() {
        let cold = RadioHardware::for_validation().into_wifi();
        let mac = &cold.radio().peripherals.ieee802154_mac;

        // Pointer inspection performs no volatile access on the host.
        assert_eq!(mac.channel().as_ptr() as usize, 0x2010_3048);
        assert_eq!(mac.ed_config().as_ptr() as usize, 0x2010_3054);
        assert_eq!(mac.event_enable().as_ptr() as usize, 0x2010_3060);
        assert_eq!(mac.rx_abort_enable().as_ptr() as usize, 0x2010_3068);
        assert_eq!(mac.coex_pti().as_ptr() as usize, 0x2010_3070);
        assert_eq!(mac.tx_abort_enable().as_ptr() as usize, 0x2010_3078);
        assert_eq!(mac.rx_status().as_ptr() as usize, 0x2010_3080);
        assert_eq!(mac.tx_status().as_ptr() as usize, 0x2010_3084);
    }
}
