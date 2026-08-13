//! Finite AP protocol/TX transaction above the shared ESP32-S31 descriptor.
//!
//! This owner is the boundary consumed by an executor: RX supplies complete
//! MPDUs, a timer supplies beacon timestamps and IRQ supplies TX progress.
//! It never calls an executor and never reports a publication as transmitted
//! before the descriptor reaches a successful terminal completion.

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{
        OrdinaryTxOutcome, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer,
    },
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_mac::tx::{LegacyRate, TxHardware};
use open_esp_radio_wifi_ap::ApServiceError;
use open_esp_radio_wpa2::frames::Wpa2TxFrame;

use crate::{
    engine::{
        Esp32s31ApEngine, Esp32s31ApEngineError, Esp32s31ApManagementOutcome,
        Esp32s31ApRuntimeHardware,
    },
    tx::{
        Esp32s31ApTx, Esp32s31ApTxClass, Esp32s31ApTxConfig, Esp32s31ApTxError, peer_legacy_rate,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPublication {
    Beacon,
    Authentication,
    Association { peer: [u8; 6], begin_wpa2: bool },
    Eapol,
    Data,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApMacReport {
    pub beacons_transmitted: u32,
    pub authentication_responses_transmitted: u32,
    pub association_responses_transmitted: u32,
    pub eapol_frames_transmitted: u32,
    pub data_frames_transmitted: u32,
    pub tx_failures: Esp32s31ApTxFailureReport,
}

/// Compact semantic classification of terminal AP TX failures.
///
/// Each counter saturates independently. Four bytes replace the former
/// undifferentiated `u32`, so retaining evidence does not enlarge the live AP
/// owner or its executor future.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApTxFailureReport {
    pub hardware_failures: u8,
    pub hardware_timeouts: u8,
    pub collision_limits: u8,
    pub last_hardware_status: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxCompletionAction {
    None,
    BeginWpa2 { peer: [u8; 6] },
    PublicationFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApMacError {
    Busy,
    Engine(Esp32s31ApEngineError),
    Transmit(Esp32s31ApTxError),
}

impl From<Esp32s31ApEngineError> for Esp32s31ApMacError {
    fn from(error: Esp32s31ApEngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<Esp32s31ApTxError> for Esp32s31ApMacError {
    fn from(error: Esp32s31ApTxError) -> Self {
        Self::Transmit(error)
    }
}

/// AP engine and the unique ordinary descriptor used by the current role.
pub struct Esp32s31ApMac<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize> {
    engine: Esp32s31ApEngine<'beacon>,
    transmit: Esp32s31ApTx<'slot, P, E, T, BUFFER_SIZE>,
    pending: Option<PendingPublication>,
    report: Esp32s31ApMacReport,
}

impl<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize>
    Esp32s31ApMac<'beacon, 'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        engine: Esp32s31ApEngine<'beacon>,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: Esp32s31ApTxConfig,
    ) -> Self {
        Self {
            engine,
            transmit: Esp32s31ApTx::new(resources, config),
            pending: None,
            report: Esp32s31ApMacReport::default(),
        }
    }

    pub const fn engine(&self) -> &Esp32s31ApEngine<'beacon> {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Esp32s31ApEngine<'beacon> {
        &mut self.engine
    }

    pub const fn report(&self) -> Esp32s31ApMacReport {
        self.report
    }

    pub const fn tx_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.engine.next_beacon_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.engine.beacon_publication_due(now_micros)
    }

    pub const fn beacon_publication_lateness(&self, now_micros: u32) -> (u32, u32) {
        self.engine.beacon_publication_lateness(now_micros)
    }

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.transmit.wait_deadline()
    }

    pub fn publish_beacon<H>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<(), Esp32s31ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let frame = self
            .engine
            .prepare_beacon(now_micros)
            .ok_or(Esp32s31ApMacError::Engine(Esp32s31ApEngineError::Beacon(
                open_esp_radio_ieee80211::beacon::ApBeaconBuildError::InvalidSequenceNumber,
            )))?;
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Beacon, frame)?;
        self.pending = Some(PendingPublication::Beacon);
        Ok(())
    }

    pub fn publish_management<H>(
        &mut self,
        hardware: &mut H,
        request: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        scratch: &mut [u8],
    ) -> Result<Esp32s31ApManagementOutcome, Esp32s31ApMacError>
    where
        H: Esp32s31ApRuntimeHardware + TxHardware,
    {
        self.require_idle()?;
        let outcome = self.engine.handle_management(
            hardware,
            request,
            authenticator_nonce,
            initial_replay_counter,
            scratch,
        )?;
        let Esp32s31ApManagementOutcome::Response { len, begin_wpa2 } = outcome else {
            return Ok(outcome);
        };
        let peer = scratch[4..10]
            .try_into()
            .expect("AP response encoder always writes receiver address");
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Management, &scratch[..len])?;
        self.pending = Some(if begin_wpa2 {
            PendingPublication::Association { peer, begin_wpa2 }
        } else if scratch[0] == 0xb0 {
            PendingPublication::Authentication
        } else {
            PendingPublication::Association { peer, begin_wpa2 }
        });
        Ok(outcome)
    }

    pub fn publish_eapol<H, const N: usize>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        frame: &Wpa2TxFrame<N>,
        scratch: &mut [u8],
    ) -> Result<(), Esp32s31ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let len = self.engine.encode_eapol(peer, frame, scratch)?;
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Eapol, &scratch[..len])?;
        self.pending = Some(PendingPublication::Eapol);
        Ok(())
    }

    pub fn publish_ethernet<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
        scratch: &mut [u8],
    ) -> Result<(), Esp32s31ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let encoded = self
            .engine
            .encode_protected_ethernet(peer, ethernet, scratch)?;
        let rate = if peer[0] & 1 != 0 {
            // Group traffic must be decodable by every associated peer. The
            // initial B/G ERP AP advertises 1 Mbit/s as a basic rate; unlike
            // unicast there is no single peer rate negotiation or ACK.
            LegacyRate::Dsss1MLong
        } else {
            self.engine
                .peer_status(peer)
                .map(|status| peer_legacy_rate(status.maximum_legacy_rate_500kbps))
                .ok_or(Esp32s31ApMacError::Engine(Esp32s31ApEngineError::Service(
                    ApServiceError::UnknownPeer,
                )))?
        };
        self.transmit.start_protected_encoded(
            hardware,
            &scratch[..encoded.length],
            encoded.hardware_key_selector,
            rate,
        )?;
        self.pending = Some(PendingPublication::Data);
        Ok(())
    }

    pub async fn service_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<(WifiTxProgress, Esp32s31ApTxCompletionAction), Esp32s31ApMacError> {
        if self.pending.is_none() {
            return Err(Esp32s31ApMacError::Busy);
        }
        let progress = self.transmit.service(hardware, wake).await?;
        if progress == WifiTxProgress::Pending {
            return Ok((progress, Esp32s31ApTxCompletionAction::None));
        }
        let pending = self.pending.take().expect("checked AP publication");
        let outcome = self
            .transmit
            .take_last_outcome()
            .expect("terminal AP TX retains an outcome");
        if !matches!(outcome, OrdinaryTxOutcome::Success(_)) {
            match outcome {
                OrdinaryTxOutcome::HardwareFailure(report) => {
                    self.report.tx_failures.hardware_failures =
                        self.report.tx_failures.hardware_failures.saturating_add(1);
                    self.report.tx_failures.last_hardware_status = report
                        .completion
                        .map(|completion| completion.status)
                        .unwrap_or(0);
                }
                OrdinaryTxOutcome::HardwareTimeout(_) => {
                    self.report.tx_failures.hardware_timeouts =
                        self.report.tx_failures.hardware_timeouts.saturating_add(1);
                }
                OrdinaryTxOutcome::CollisionLimit(_) => {
                    self.report.tx_failures.collision_limits =
                        self.report.tx_failures.collision_limits.saturating_add(1);
                }
                OrdinaryTxOutcome::Success(_) => unreachable!("checked failed AP publication"),
            }
            return Ok((progress, Esp32s31ApTxCompletionAction::PublicationFailed));
        }
        let action = match pending {
            PendingPublication::Beacon => {
                self.report.beacons_transmitted = self.report.beacons_transmitted.saturating_add(1);
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Authentication => {
                self.report.authentication_responses_transmitted = self
                    .report
                    .authentication_responses_transmitted
                    .saturating_add(1);
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Association { peer, begin_wpa2 } => {
                self.report.association_responses_transmitted = self
                    .report
                    .association_responses_transmitted
                    .saturating_add(1);
                if begin_wpa2 {
                    Esp32s31ApTxCompletionAction::BeginWpa2 { peer }
                } else {
                    Esp32s31ApTxCompletionAction::None
                }
            }
            PendingPublication::Eapol => {
                self.report.eapol_frames_transmitted =
                    self.report.eapol_frames_transmitted.saturating_add(1);
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Data => {
                self.report.data_frames_transmitted =
                    self.report.data_frames_transmitted.saturating_add(1);
                Esp32s31ApTxCompletionAction::None
            }
        };
        Ok((progress, action))
    }

    fn require_idle(&self) -> Result<(), Esp32s31ApMacError> {
        if self.pending.is_some() {
            Err(Esp32s31ApMacError::Busy)
        } else {
            Ok(())
        }
    }

    /// Recover AP protocol and common TX resources only after TX is idle.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_into_parts(
        self,
    ) -> Result<
        (
            Esp32s31ApEngine<'beacon>,
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            Esp32s31ApMacReport,
        ),
        Self,
    > {
        let Self {
            engine,
            transmit,
            pending,
            report,
        } = self;
        if pending.is_some() {
            return Err(Self {
                engine,
                transmit,
                pending,
                report,
            });
        }
        match transmit.try_into_resources() {
            Ok(resources) => Ok((engine, resources, report)),
            Err(transmit) => Err(Self {
                engine,
                transmit,
                pending,
                report,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::pin,
        task::{Context, Poll},
    };

    use open_esp_radio_esp32s31_pac::{
        MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome,
        MacTxDetachReason, MacTxQueueDetached,
    };
    use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;
    use open_esp_radio_esp32s31_wifi_mac::{
        ap_policy::ApRxPolicyHardware,
        crypto::CcmpKeyHardware,
        tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot},
        tx_runtime::WifiTxRuntimePolicy,
    };
    use open_esp_radio_ieee80211::{beacon::WPA2_BEACON_CAPACITY, ssid::WifiSsid};
    use open_esp_radio_wifi_ap::AccessPointService;
    use open_esp_radio_wpa2::{Pmk, frames::Wpa2Gtk};

    use super::*;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(core::task::Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Default)]
    struct Hardware {
        completion: Option<MacTxCompletionRegisters>,
    }

    impl ApRxPolicyHardware for Hardware {
        fn apply_ap_link_policy(&mut self, _address: [u8; 6]) {}
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, _index: u8) {}
    }

    impl Esp32s31ApRuntimeHardware for Hardware {
        fn stop_ap_tsf(&mut self) {}
    }

    impl TxHardware for Hardware {
        fn prepare_bound_legacy_tx(
            &mut self,
            _dma: &dyn PreparedTxDma,
            _queue: u8,
            _program: MacLegacyTxProgram,
        ) -> bool {
            true
        }

        fn start_bound_legacy_tx(
            &mut self,
            _dma: &dyn HardwareOwnedTxDma,
            _queue: u8,
            _plcp0: u32,
        ) {
        }

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            self.completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            false
        }

        fn with_tx_queue_detached<R>(
            &mut self,
            _queue: u8,
            descriptor: u32,
            reason: MacTxDetachReason,
            detached: impl for<'a> FnOnce(MacTxQueueDetached<'a>) -> R,
        ) -> MacTxDetachOutcome<R> {
            match reason {
                MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                    MacTxQueueDetached::new_model(descriptor),
                )),
                _ => MacTxDetachOutcome::NoEvent,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct Power;

    impl WifiTxPowerProfile for Power {
        fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: 1,
                alternate: 1,
            }
        }
    }

    struct Timer;

    impl WifiTxTimer for Timer {
        fn now_micros(&self) -> u64 {
            0
        }

        fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }

        fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    #[test]
    fn prepared_beacon_becomes_evidence_only_after_terminal_success() {
        let ap = [2, 0, 0, 0, 0, 1];
        let mut hardware = Hardware::default();
        let mut beacon = [0; WPA2_BEACON_CAPACITY];
        let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
        let mut pairwise = crate::security::Esp32s31ApPairwiseKeyStorage::new();
        let engine = Esp32s31ApEngine::start(
            &mut hardware,
            AccessPointService::new(
                ap,
                Pmk::derive(b"password", b"ap").unwrap(),
                Wpa2Gtk::new(1, true, [7; 16]).unwrap(),
                open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
                &mut peers,
            ),
            &mut beacon,
            &mut pairwise,
            &WifiSsid::new(b"ap").unwrap(),
            6,
            100,
            2,
        )
        .unwrap_or_else(|_| panic!("AP engine starts"));
        let mut slot = pin!(TxSlot::<512>::new_model());
        let mut mac = Esp32s31ApMac::new(
            engine,
            WifiTxResources {
                slot: slot.as_mut(),
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy: || 0,
                timer: Timer,
            },
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1_000,
            },
        );

        mac.publish_beacon(&mut hardware, 102_400).unwrap();
        assert_eq!(mac.report().beacons_transmitted, 0);
        hardware.completion = Some(MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: 0,
            alternate: 0,
            trigger_flow: false,
        });
        let (progress, action) = block_on(mac.service_tx(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
            },
        ))
        .unwrap();
        assert_eq!(progress, WifiTxProgress::Complete);
        assert_eq!(action, Esp32s31ApTxCompletionAction::None);
        assert_eq!(mac.report().beacons_transmitted, 1);
        assert!(mac.try_into_parts().is_ok());
    }
}
