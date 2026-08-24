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
use open_esp_radio_esp32s31_wifi_mac::tx::{HtRate, LegacyRate, TxHardware};
use open_esp_radio_ieee80211::ap::ApPeerDisconnectKind;
use open_esp_radio_ieee80211::block_ack::TxBlockAckAlarm;
use open_esp_radio_wifi_ap::{ApPeerClose, ApPeerPowerState, ApServiceError};
use open_esp_radio_wpa2::frames::Wpa2TxFrame;

use crate::{
    ampdu::Esp32s31ApAggregateAdmission,
    engine::{
        Esp32s31ApEngine, Esp32s31ApEngineError, Esp32s31ApManagementOutcome,
        Esp32s31ApRuntimeHardware,
    },
    tx::{
        Esp32s31ApTx, Esp32s31ApTxClass, Esp32s31ApTxConfig, Esp32s31ApTxError, Esp32s31ApTxParked,
        peer_ht_rate, peer_legacy_rate,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPublication {
    Beacon,
    Authentication,
    Association {
        peer: [u8; 6],
        begin_wpa2: bool,
    },
    Eapol {
        peer: [u8; 6],
        retransmission: bool,
    },
    Data {
        peer: [u8; 6],
    },
    BlockAckRequest {
        peer: [u8; 6],
    },
    RxBlockAckResponse,
    PeerDisconnect {
        close: ApPeerClose,
        stage: Esp32s31ApPeerDisconnectStage,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApPeerDisconnectStage {
    Disassociation,
    Deauthentication,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApMacObservation {
    pub beacons_transmitted: u32,
    pub authentication_responses_transmitted: u32,
    pub association_responses_transmitted: u32,
    pub eapol_frames_transmitted: u32,
    pub data_frames_transmitted: u32,
    pub rx_block_ack_responses_transmitted: u32,
    pub data_tx: Esp32s31ApDataTxObservation,
    /// Disconnect transactions accepted by the hardware TX owner.
    pub disassociations_published: u32,
    pub deauthentications_published: u32,
    /// Disconnect transactions whose terminal completion reported an ACK.
    pub disassociations_acknowledged: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_failures: Esp32s31ApTxFailureObservation,
}

/// Aggregate terminal evidence for AP data-frame TX transactions.
///
/// This is observation only: it consumes the already-decoded ordinary TX
/// outcome and does not participate in retry, rate or queue decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApDataTxObservation {
    pub attempts: u32,
    pub retried_frames: u32,
    pub cts_timeout_retries: u32,
    pub ack_timeout_retries: u32,
    pub collision_retries: u32,
    pub maximum_attempts: u8,
    pub minimum_final_rate_kbps: u32,
    pub ack_snr_samples: u32,
    pub minimum_ack_snr_db: i8,
    pub maximum_ack_snr_db: i8,
}

#[cfg(any(feature = "diagnostics", test))]
impl Esp32s31ApDataTxObservation {
    fn observe(&mut self, report: open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxReport) {
        let attempts = report.status.attempts;
        self.attempts = self.attempts.saturating_add(u32::from(attempts));
        self.retried_frames = self.retried_frames.saturating_add(u32::from(attempts > 1));
        self.cts_timeout_retries = self
            .cts_timeout_retries
            .saturating_add(u32::from(report.retries.cts_timeouts));
        self.ack_timeout_retries = self
            .ack_timeout_retries
            .saturating_add(u32::from(report.retries.ack_timeouts));
        self.collision_retries = self
            .collision_retries
            .saturating_add(u32::from(report.retries.collisions));
        self.maximum_attempts = self.maximum_attempts.max(attempts);
        let final_rate_kbps = report.status.final_rate.nominal_kbps();
        if self.minimum_final_rate_kbps == 0 || final_rate_kbps < self.minimum_final_rate_kbps {
            self.minimum_final_rate_kbps = final_rate_kbps;
        }
        if let Some(ack_snr_db) = report.status.ack_snr_db {
            if self.ack_snr_samples == 0 {
                self.minimum_ack_snr_db = ack_snr_db;
                self.maximum_ack_snr_db = ack_snr_db;
            } else {
                self.minimum_ack_snr_db = self.minimum_ack_snr_db.min(ack_snr_db);
                self.maximum_ack_snr_db = self.maximum_ack_snr_db.max(ack_snr_db);
            }
            self.ack_snr_samples = self.ack_snr_samples.saturating_add(1);
        }
    }
}

/// Compact semantic classification of terminal AP TX failures.
///
/// Each counter saturates independently. Four bytes replace the former
/// undifferentiated `u32`, so retaining evidence does not enlarge the live AP
/// owner or its executor future.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ApTxFailureObservation {
    pub hardware_failures: u8,
    pub hardware_timeouts: u8,
    pub collision_limits: u8,
    pub last_hardware_status: u8,
}

/// Diagnostic state owned by the optional AP MAC observer.
///
/// This owner is absent from ordinary builds. It consumes already-decoded TX
/// outcomes and never participates in publication, retry or rate decisions.
#[cfg(any(feature = "diagnostics", test))]
#[derive(Default)]
struct Esp32s31ApMacObserver {
    observation: Esp32s31ApMacObservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxCompletionAction {
    None,
    BeginWpa2 {
        peer: [u8; 6],
    },
    PublicationFailed,
    PeerDisconnectTerminal {
        close: ApPeerClose,
        stage: Esp32s31ApPeerDisconnectStage,
        acknowledged: bool,
    },
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
    block_ack_alarm: Option<([u8; 6], TxBlockAckAlarm)>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Esp32s31ApMacObserver,
}

/// AP protocol state with no ordinary descriptor or DMA publication owner.
pub struct Esp32s31ApMacParked<'beacon> {
    engine: Esp32s31ApEngine<'beacon>,
    transmit: Esp32s31ApTxParked,
    block_ack_alarm: Option<([u8; 6], TxBlockAckAlarm)>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Esp32s31ApMacObserver,
}

/// Quiescent AP MAC owners returned after the ordinary TX transaction parks.
pub struct Esp32s31ApMacParts<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub engine: Esp32s31ApEngine<'beacon>,
    pub transmit: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
}

impl Esp32s31ApMacParked<'_> {
    /// Observe the role-local beacon schedule without recovering the shared
    /// ordinary-TX capability.
    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.engine.beacon_publication_due(now_micros)
    }

    /// Return the current beacon deadline while the physical TX owner is lent
    /// to neither role.
    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.engine.next_beacon_delay(now_micros)
    }

    /// Earliest AP protocol deadline which may require a future hardware
    /// publication. This is observation only and grants no MMIO authority.
    pub fn next_control_deadline(&self) -> Option<u64> {
        self.engine
            .next_peer_deadline()
            .into_iter()
            .chain(self.engine.next_wpa2_retry_deadline())
            .chain(self.block_ack_alarm.map(|(_, alarm)| alarm.deadline_us))
            .min()
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.engine.has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.engine.smallest_operational_tx_block_ack_window()
    }
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
            block_ack_alarm: None,
            #[cfg(any(feature = "diagnostics", test))]
            observer: Esp32s31ApMacObserver::default(),
        }
    }

    pub const fn engine(&self) -> &Esp32s31ApEngine<'beacon> {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Esp32s31ApEngine<'beacon> {
        &mut self.engine
    }

    /// Borrow the AP encoder/key owner and the ordinary publication-policy
    /// adapter together at an idle boundary. The shared retained-DMA A-MPDU
    /// owner consumes these two narrow capabilities; it does not receive the
    /// complete AP MAC or peer table.
    pub fn try_aggregate_adapter(
        &mut self,
    ) -> Result<
        (
            &mut Esp32s31ApEngine<'beacon>,
            &mut Esp32s31ApTx<'slot, P, E, T, BUFFER_SIZE>,
        ),
        Esp32s31ApMacError,
    > {
        self.require_idle()?;
        Ok((&mut self.engine, &mut self.transmit))
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> Esp32s31ApMacObservation {
        self.observer.observation
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
        now_micros: u64,
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
            now_micros,
            scratch,
        )?;
        if let Some(peer) = request
            .get(10..16)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
            && self.engine.tx_block_ack_agreement(peer).is_some()
        {
            self.block_ack_alarm = None;
        }
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
        self.pending = Some(PendingPublication::Eapol {
            peer,
            retransmission: frame.retransmission(),
        });
        Ok(())
    }

    pub fn publish_rx_block_ack_response<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        body: &[u8],
        scratch: &mut [u8],
    ) -> Result<(), Esp32s31ApMacError> {
        self.require_idle()?;
        let length = self
            .engine
            .encode_rx_block_ack_response(peer, body, scratch)?;
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Management, &scratch[..length])?;
        self.pending = Some(PendingPublication::RxBlockAckResponse);
        Ok(())
    }

    pub fn publish_tx_block_ack_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        now_micros: u64,
        scratch: &mut [u8],
    ) -> Result<bool, Esp32s31ApMacError> {
        self.require_idle()?;
        let Some((length, alarm)) = self
            .engine
            .prepare_tx_block_ack_request(peer, now_micros, scratch)?
        else {
            return Ok(false);
        };
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Management, &scratch[..length])?;
        self.block_ack_alarm = Some((peer, alarm));
        self.pending = Some(PendingPublication::BlockAckRequest { peer });
        Ok(true)
    }

    pub fn peer_ht_rate(&self, peer: [u8; 6]) -> Option<HtRate> {
        let status = self.engine.peer_status(peer)?;
        peer_ht_rate(self.engine.channel(), status.ht?)
    }

    /// Resolve one Ethernet lease to the AP peer/rate/BlockAck policy that
    /// must remain stable for the complete aggregate build.
    pub fn aggregate_admission(&self, ethernet: &[u8]) -> Option<Esp32s31ApAggregateAdmission> {
        let peer = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())?;
        if peer[0] & 1 != 0 {
            return None;
        }
        let (binding, status) = self.engine.bind_aggregate_peer(peer).ok()?;
        if status.power_state != ApPeerPowerState::Active {
            return None;
        }
        let agreement = status.tx_block_ack?;
        let rate = peer_ht_rate(self.engine.channel(), status.ht?)?;
        Some(Esp32s31ApAggregateAdmission::new(
            binding,
            rate,
            agreement.window,
        ))
    }

    pub fn next_tx_block_ack_deadline(&self) -> Option<u64> {
        self.block_ack_alarm.map(|(_, alarm)| alarm.deadline_us)
    }

    pub fn expire_tx_block_ack(&mut self, now_micros: u64) -> Result<bool, Esp32s31ApMacError> {
        let Some((peer, alarm)) = self.block_ack_alarm else {
            return Ok(false);
        };
        if now_micros < alarm.deadline_us {
            return Ok(false);
        }
        self.block_ack_alarm = None;
        Ok(self.engine.observe_tx_block_ack_alarm(peer, alarm)?)
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
        self.publish_ethernet_with_more_data(hardware, peer, ethernet, scratch, false)
    }

    /// Publish one protected network frame with an explicit AP More Data bit.
    pub fn publish_ethernet_with_more_data<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
        scratch: &mut [u8],
        more_data: bool,
    ) -> Result<(), Esp32s31ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let encoded = self
            .engine
            .encode_protected_ethernet_with_more_data(peer, ethernet, scratch, more_data)?;
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
        self.pending = Some(PendingPublication::Data { peer });
        Ok(())
    }

    pub fn publish_peer_disconnect<H>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
        stage: Esp32s31ApPeerDisconnectStage,
        scratch: &mut [u8],
    ) -> Result<(), Esp32s31ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let (kind, reason) = match stage {
            Esp32s31ApPeerDisconnectStage::Disassociation => (
                ApPeerDisconnectKind::Disassociation,
                if close.kind == open_esp_radio_wifi_ap::ApPeerCloseKind::InactivityTimeout {
                    4
                } else {
                    2
                },
            ),
            Esp32s31ApPeerDisconnectStage::Deauthentication => {
                (ApPeerDisconnectKind::Deauthentication, 2)
            }
        };
        let length = self
            .engine
            .encode_peer_disconnect(close, kind, reason, scratch)?;
        // Complete vendor `wifi_softap_stop` publishes subtype 0xa0 and then
        // 0xc0 before `cnx_node_leave`; neither call waits for an ACK. Keep our
        // stronger DMA-ownership rule (each transaction reaches a terminal
        // completion before the next one), but distinguish hardware
        // publication from an ACK that a departing peer may never return.
        self.transmit
            .start_encoded(hardware, Esp32s31ApTxClass::Management, &scratch[..length])?;
        #[cfg(any(feature = "diagnostics", test))]
        {
            match stage {
                Esp32s31ApPeerDisconnectStage::Disassociation => {
                    self.observer.observation.disassociations_published = self
                        .observer
                        .observation
                        .disassociations_published
                        .saturating_add(1);
                }
                Esp32s31ApPeerDisconnectStage::Deauthentication => {
                    self.observer.observation.deauthentications_published = self
                        .observer
                        .observation
                        .deauthentications_published
                        .saturating_add(1);
                }
            }
        }
        self.pending = Some(PendingPublication::PeerDisconnect { close, stage });
        Ok(())
    }

    pub async fn service_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        now_micros: u64,
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
        #[cfg(any(feature = "diagnostics", test))]
        if let PendingPublication::Data { peer } = pending
            && peer[0] & 1 == 0
        {
            // Group traffic has no ACK and deliberately uses the 1-Mbit/s
            // basic rate. Mixing it into per-peer retry/rate evidence makes a
            // healthy broadcast look like a unicast rate-control collapse.
            self.observer.observation.data_tx.observe(outcome.report());
        }
        if !matches!(outcome, OrdinaryTxOutcome::Success(_)) {
            #[cfg(any(feature = "diagnostics", test))]
            match outcome {
                OrdinaryTxOutcome::HardwareFailure(report) => {
                    self.observer.observation.tx_failures.hardware_failures = self
                        .observer
                        .observation
                        .tx_failures
                        .hardware_failures
                        .saturating_add(1);
                    self.observer.observation.tx_failures.last_hardware_status = report
                        .completion
                        .map(|completion| completion.status)
                        .unwrap_or(0);
                }
                OrdinaryTxOutcome::HardwareTimeout(_) => {
                    self.observer.observation.tx_failures.hardware_timeouts = self
                        .observer
                        .observation
                        .tx_failures
                        .hardware_timeouts
                        .saturating_add(1);
                }
                OrdinaryTxOutcome::CollisionLimit(_) => {
                    self.observer.observation.tx_failures.collision_limits = self
                        .observer
                        .observation
                        .tx_failures
                        .collision_limits
                        .saturating_add(1);
                }
                OrdinaryTxOutcome::Success(_) => unreachable!("checked failed AP publication"),
            }
            return Ok((
                progress,
                match pending {
                    PendingPublication::PeerDisconnect { close, stage } => {
                        Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                            close,
                            stage,
                            acknowledged: false,
                        }
                    }
                    PendingPublication::Eapol {
                        peer,
                        retransmission,
                    } => {
                        self.engine.observe_wpa2_transmit(
                            peer,
                            retransmission,
                            false,
                            now_micros,
                        )?;
                        Esp32s31ApTxCompletionAction::PublicationFailed
                    }
                    _ => Esp32s31ApTxCompletionAction::PublicationFailed,
                },
            ));
        }
        let action = match pending {
            PendingPublication::Beacon => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.beacons_transmitted = self
                        .observer
                        .observation
                        .beacons_transmitted
                        .saturating_add(1);
                }
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Authentication => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer
                        .observation
                        .authentication_responses_transmitted = self
                        .observer
                        .observation
                        .authentication_responses_transmitted
                        .saturating_add(1);
                }
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Association { peer, begin_wpa2 } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.association_responses_transmitted = self
                        .observer
                        .observation
                        .association_responses_transmitted
                        .saturating_add(1);
                }
                if begin_wpa2 {
                    Esp32s31ApTxCompletionAction::BeginWpa2 { peer }
                } else {
                    Esp32s31ApTxCompletionAction::None
                }
            }
            PendingPublication::Eapol {
                peer,
                retransmission,
            } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.eapol_frames_transmitted = self
                        .observer
                        .observation
                        .eapol_frames_transmitted
                        .saturating_add(1);
                }
                self.engine
                    .observe_wpa2_transmit(peer, retransmission, true, now_micros)?;
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::Data { peer } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.data_frames_transmitted = self
                        .observer
                        .observation
                        .data_frames_transmitted
                        .saturating_add(1);
                }
                if peer[0] & 1 == 0 {
                    self.engine.observe_peer_activity(peer, now_micros)?;
                }
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::BlockAckRequest { .. } => Esp32s31ApTxCompletionAction::None,
            PendingPublication::RxBlockAckResponse => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.rx_block_ack_responses_transmitted = self
                        .observer
                        .observation
                        .rx_block_ack_responses_transmitted
                        .saturating_add(1);
                }
                Esp32s31ApTxCompletionAction::None
            }
            PendingPublication::PeerDisconnect { close, stage } => {
                #[cfg(any(feature = "diagnostics", test))]
                match stage {
                    Esp32s31ApPeerDisconnectStage::Disassociation => {
                        self.observer.observation.disassociations_acknowledged = self
                            .observer
                            .observation
                            .disassociations_acknowledged
                            .saturating_add(1);
                    }
                    Esp32s31ApPeerDisconnectStage::Deauthentication => {
                        self.observer.observation.deauthentications_acknowledged = self
                            .observer
                            .observation
                            .deauthentications_acknowledged
                            .saturating_add(1);
                    }
                }
                Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                    close,
                    stage,
                    acknowledged: true,
                }
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

    /// Lend the physical ordinary TX owner while preserving all AP-local
    /// protocol, timer and observation state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            Esp32s31ApMacParked<'beacon>,
        ),
        Self,
    > {
        let Self {
            engine,
            transmit,
            pending,
            block_ack_alarm,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        } = self;
        if pending.is_some() {
            return Err(Self {
                engine,
                transmit,
                pending,
                block_ack_alarm,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
            });
        }
        match transmit.try_park() {
            Ok((resources, transmit)) => Ok((
                resources,
                Esp32s31ApMacParked {
                    engine,
                    transmit,
                    block_ack_alarm,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                },
            )),
            Err(transmit) => Err(Self {
                engine,
                transmit,
                pending: None,
                block_ack_alarm,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
            }),
        }
    }

    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: Esp32s31ApMacParked<'beacon>,
    ) -> Self {
        let Esp32s31ApMacParked {
            engine,
            transmit,
            block_ack_alarm,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        } = parked;
        Self {
            engine,
            transmit: Esp32s31ApTx::resume(resources, transmit),
            pending: None,
            block_ack_alarm,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        }
    }

    /// Recover AP protocol and common TX resources only after TX is idle.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_into_parts(
        self,
    ) -> Result<Esp32s31ApMacParts<'beacon, 'slot, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, parked)| {
            let Esp32s31ApMacParked { engine, .. } = parked;
            Esp32s31ApMacParts {
                engine,
                transmit: resources,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::pin,
        task::{Context, Poll},
    };

    use open_esp_radio_esp32s31_hal::types::{
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

    impl open_esp_radio_esp32s31_wifi_mac::ap_tsf::ApTsfHardware for Hardware {
        fn reset_and_start_access_point_tsf(&mut self) {}

        fn stop_access_point_tsf(&mut self) {}
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
                open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
                &mut peers,
            ),
            &mut beacon,
            &mut pairwise,
            &WifiSsid::new(b"ap").unwrap(),
            open_esp_radio_ieee80211::channel::WifiChannel::mhz20(6).unwrap(),
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
        assert_eq!(mac.observation().beacons_transmitted, 0);
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
            1,
        ))
        .unwrap();
        assert_eq!(progress, WifiTxProgress::Complete);
        assert_eq!(action, Esp32s31ApTxCompletionAction::None);
        assert_eq!(mac.observation().beacons_transmitted, 1);
        assert!(mac.try_into_parts().is_ok());
    }
}
