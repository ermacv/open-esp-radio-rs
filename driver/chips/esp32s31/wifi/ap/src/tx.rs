//! ESP32-S31 ordinary TX ownership specialized for an access-point epoch.
//!
//! Frame construction remains in the portable AP/IEEE 802.11 layers. This
//! owner adds the chip queue, retry, power and descriptor policy and exposes
//! the same finite IRQ/deadline transaction used by STA. It deliberately does
//! not poll, spawn tasks or count a frame as transmitted before completion.

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan,
        TX_CCMP_MIC_SIZE, TX_METADATA_SIZE, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources,
        WifiTxTimer,
    },
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_mac::tx::{LegacyRate, LegacyTxQueue, TxHardware, TxPhyRate};
use open_esp_radio_wifi_softmac::{MacTxPlan, MacTxQueueState};

/// Runtime-independent publication policy for the initial AP implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApTxConfig {
    /// Watchdog applied independently to each hardware publication.
    pub publication_timeout_micros: u64,
}

/// Semantic AP frame class translated into the private ESP32-S31 queue plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxClass {
    /// Broadcast beacon; never retried for an acknowledgement.
    Beacon,
    /// Authentication, association or deauthentication response.
    Management,
    /// Unprotected EAPOL exchange during the four-way handshake.
    Eapol,
    /// Pairwise protected Ethernet data after the controlled port opens.
    Data,
}

impl Esp32s31ApTxClass {
    fn publication_limit(self, rate: LegacyRate) -> u8 {
        match self {
            Self::Beacon => 1,
            Self::Management | Self::Eapol | Self::Data => rate
                .vendor_retry_publication_limit()
                .expect("every AP legacy rate has a recovered Dot11G schedule"),
        }
    }

    const fn queue(self) -> LegacyTxQueue {
        // The recovered vendor management/control path uses VO. EAPOL is
        // deliberately kept on the same bounded pre-data path until the
        // controlled port opens.
        match self {
            Self::Beacon | Self::Management | Self::Eapol => LegacyTxQueue::Voice,
            Self::Data => LegacyTxQueue::BestEffort,
        }
    }

    const fn initial_rate(self) -> LegacyRate {
        match self {
            // AP v1 advertises an ERP BSS, so every associated peer supports
            // the mandatory 24 Mbit/s OFDM rate. Keep discovery and controlled
            // port setup on the maximally compatible 1 Mbit/s path, but do
            // not serialize ordinary data at that management rate.
            Self::Data => LegacyRate::Ofdm24M,
            Self::Beacon | Self::Management | Self::Eapol => LegacyRate::Dsss1MLong,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxError {
    FrameTooLarge,
    Ordinary(OrdinaryTxError),
}

impl From<OrdinaryTxError> for Esp32s31ApTxError {
    fn from(error: OrdinaryTxError) -> Self {
        Self::Ordinary(error)
    }
}

/// Unique ordinary descriptor owned by one active AP epoch.
#[must_use = "an AP TX owner must be quiesced and recovered before role transition"]
pub struct Esp32s31ApTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: Esp32s31ApTxConfig,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31ApTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: Esp32s31ApTxConfig,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config,
        }
    }

    pub fn queue_state(&self) -> MacTxQueueState {
        self.ordinary.queue_state()
    }

    /// Copy one portable encoded MPDU into the only ordinary DMA slot and
    /// publish it. A successful return means hardware owns the descriptor,
    /// not that the frame reached the air.
    pub fn start_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: Esp32s31ApTxClass,
        frame: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(hardware, class, frame, 0, 0, None)
    }

    /// Publish one plaintext protected MPDU through hardware CCMP.
    pub fn start_protected_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        hardware_key_selector: u8,
        rate: LegacyRate,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(
            hardware,
            Esp32s31ApTxClass::Data,
            frame,
            TX_CCMP_MIC_SIZE,
            hardware_key_selector,
            Some(rate),
        )
    }

    fn start_encoded_with_key<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: Esp32s31ApTxClass,
        frame: &[u8],
        hardware_mic_length: usize,
        hardware_key_selector: u8,
        rate: Option<LegacyRate>,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        let end = TX_METADATA_SIZE
            .checked_add(frame.len())
            .ok_or(Esp32s31ApTxError::FrameTooLarge)?;
        let buffer = self.ordinary.buffer_mut()?;
        let destination = buffer
            .get_mut(TX_METADATA_SIZE..end)
            .ok_or(Esp32s31ApTxError::FrameTooLarge)?;
        destination.copy_from_slice(frame);

        let queue = class.queue();
        let initial_rate = rate.unwrap_or_else(|| class.initial_rate());
        Ok(self.ordinary.start(
            hardware,
            OrdinaryTxPlan {
                frame_length: frame.len(),
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: queue.access_category(),
                    initial_rate: TxPhyRate::Legacy(initial_rate),
                    publication_limit: class.publication_limit(initial_rate),
                    publication_timeout_micros: self.config.publication_timeout_micros,
                },
                hardware_mic_length,
                hardware_key_selector,
                interface: OrdinaryTxInterface::AccessPoint,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
            },
        )?)
    }

    /// Advance an active descriptor from one coalesced IRQ/deadline edge.
    pub async fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        Ok(self.ordinary.service(hardware, wake).await?)
    }

    pub fn take_last_outcome(&mut self) -> Option<OrdinaryTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub fn wait_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    /// Recover the common TX capability only when no descriptor is owned by
    /// DMA and the queue is not quarantined for radio reset.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(self) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        let Self { ordinary, config } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok(resources),
            Err(ordinary) => Err(Self { ordinary, config }),
        }
    }
}

/// Translate the portable 500-kbit/s association rate into the exact legacy
/// hardware rate enum. The parser supplies only members of the AP's advertised
/// B/G rate set; the 1-Mbit/s fallback preserves safety for stale peer state.
pub const fn peer_legacy_rate(maximum_rate_500kbps: u8) -> LegacyRate {
    match maximum_rate_500kbps {
        108 => LegacyRate::Ofdm54M,
        96 => LegacyRate::Ofdm48M,
        72 => LegacyRate::Ofdm36M,
        48 => LegacyRate::Ofdm24M,
        36 => LegacyRate::Ofdm18M,
        24 => LegacyRate::Ofdm12M,
        18 => LegacyRate::Ofdm9M,
        12 => LegacyRate::Ofdm6M,
        22 => LegacyRate::Cck11MLong,
        11 => LegacyRate::Cck5M5Long,
        4 => LegacyRate::Dsss2MLong,
        _ => LegacyRate::Dsss1MLong,
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
        MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome, MacTxDetachReason,
        MacTxQueueDetached,
    };
    use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;
    use open_esp_radio_esp32s31_wifi_mac::{
        MacInterface,
        tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot},
        tx_runtime::WifiTxRuntimePolicy,
    };

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
        prepare: bool,
        publications: u8,
        legacy_program: Option<MacLegacyTxProgram>,
        completion: Option<MacTxCompletionRegisters>,
    }

    impl TxHardware for Hardware {
        fn prepare_bound_legacy_tx(
            &mut self,
            _dma: &dyn PreparedTxDma,
            _queue: u8,
            program: MacLegacyTxProgram,
        ) -> bool {
            self.legacy_program = Some(program);
            self.prepare
        }

        fn start_bound_legacy_tx(
            &mut self,
            _dma: &dyn HardwareOwnedTxDma,
            _queue: u8,
            _plcp0: u32,
        ) {
            self.publications += 1;
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
            expected_descriptor_head: u32,
            reason: MacTxDetachReason,
            detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
        ) -> MacTxDetachOutcome<R> {
            match reason {
                MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                    MacTxQueueDetached::new_model(expected_descriptor_head),
                )),
                MacTxDetachReason::Timeout | MacTxDetachReason::Collision => {
                    MacTxDetachOutcome::NoEvent
                }
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
            1
        }

        fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }

        fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    #[test]
    fn peer_rate_mapping_preserves_every_advertised_bg_rate() {
        assert_eq!(Esp32s31ApTxClass::Data.initial_rate(), LegacyRate::Ofdm24M);
        assert_eq!(peer_legacy_rate(108), LegacyRate::Ofdm54M);
        assert_eq!(peer_legacy_rate(96), LegacyRate::Ofdm48M);
        assert_eq!(peer_legacy_rate(72), LegacyRate::Ofdm36M);
        assert_eq!(peer_legacy_rate(48), LegacyRate::Ofdm24M);
        assert_eq!(peer_legacy_rate(22), LegacyRate::Cck11MLong);
        assert_eq!(peer_legacy_rate(0), LegacyRate::Dsss1MLong);
        for class in [
            Esp32s31ApTxClass::Beacon,
            Esp32s31ApTxClass::Management,
            Esp32s31ApTxClass::Eapol,
        ] {
            assert_eq!(class.initial_rate(), LegacyRate::Dsss1MLong);
        }
        assert_eq!(
            Esp32s31ApTxClass::Beacon.publication_limit(LegacyRate::Dsss1MLong),
            1
        );
        assert_eq!(
            Esp32s31ApTxClass::Management.publication_limit(LegacyRate::Dsss1MLong),
            32
        );
        assert_eq!(
            Esp32s31ApTxClass::Data.publication_limit(LegacyRate::Ofdm54M),
            32
        );
    }

    #[test]
    fn beacon_is_one_publication_and_resources_return_only_after_completion() {
        let mut slot = pin!(TxSlot::<256>::new_model());
        let resources = WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: Timer,
        };
        let mut tx = Esp32s31ApTx::new(
            resources,
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1_000,
            },
        );
        let mut hardware = Hardware {
            prepare: true,
            completion: Some(MacTxCompletionRegisters {
                aux_a: 0,
                aux_b: 0,
                aux_c: 0,
                primary: 0,
                alternate: 0,
                trigger_flow: false,
            }),
            ..Hardware::default()
        };
        let mut beacon = [0; 24];
        beacon[4] = 0xff;
        assert_eq!(
            tx.start_encoded(&mut hardware, Esp32s31ApTxClass::Beacon, &beacon),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
        assert_eq!(hardware.publications, 1);
        assert_eq!(
            hardware.legacy_program.unwrap().interface,
            MacInterface::AccessPoint
        );

        let progress = block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
            },
        ));
        assert_eq!(progress, Ok(WifiTxProgress::Complete));
        assert!(tx.take_last_outcome().unwrap().is_success());
        assert!(tx.try_into_resources().is_ok());
    }
}
