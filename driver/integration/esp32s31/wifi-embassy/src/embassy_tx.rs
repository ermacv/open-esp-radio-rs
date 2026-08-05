//! Safe ownership bridge from pinned `embassy-net` frames to S31 TX DMA.

#![allow(unsafe_code, reason = "referenced TX DMA ownership boundary")]

use core::{mem, pin::Pin};

use open_esp_radio_embassy_net::{PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi_lmac::{
    tx::{
        HeAmpduTxConfig, HeEdcaTxopLimit, HeRate, HtAmpduDensity, HtAmpduTxConfig, HtRate,
        LegacyTxQueue, TxCookie, TxPhyRate, TxSlotState,
    },
    tx_ampdu::{
        HtAmpduHardware, HtAmpduLength, HtAmpduTxCompletion, HtAmpduTxError, HtAmpduTxStorage,
        TX_AMPDU_METADATA_SIZE,
    },
};
use open_esp_radio_ieee80211::{
    data::DataHeControl,
    station::{
        EncodedStaFrame, StaProtectedEthernetFrame, StationFrameError,
        sta_protected_amsdu_pair_frame_length,
    },
};

/// Initial queue-admission policy for a referenced A-MPDU transaction.
///
/// HT needs two MPDUs before it can publish the aggregate format used by this
/// adapter. HE can publish one MPDU, so it must claim only the first pinned
/// network lease initially. [`ReferencedHtAmpduBatch::can_push_he`] then
/// checks the exact rate/TXOP APEP ceiling before the caller removes another
/// lease from its queue.
///
/// This distinction is observable at low DCM rates: MCS0 DCM admits one
/// full-size Ethernet frame under the ROM-derived 1,850-byte APEP limit but
/// not two. Prefetching the second lease before that check previously forced
/// it through an unrelated legacy spill path.
///
/// SOURCE: ROM rev0 `he_max_apep_length`, complete
/// `libpp.a[pp_he.o]::ppCheckTxHEAMPDUlength`, and
/// HIL_OPEN_HE20_DCM_CONNECTED_2026_07_31.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferencedAmpduIngressPolicy {
    HtPrefetchPair,
    HeStartSingleThenCheck,
}

impl ReferencedAmpduIngressPolicy {
    pub const fn for_rate(rate: TxPhyRate) -> Option<Self> {
        match rate {
            TxPhyRate::Legacy(_) => None,
            TxPhyRate::Ht(_) => Some(Self::HtPrefetchPair),
            TxPhyRate::He(_) => Some(Self::HeStartSingleThenCheck),
        }
    }

    /// Whether the outer queue owner may claim a second lease before the
    /// referenced batch has performed its per-frame capacity check.
    pub const fn prefetch_second(self) -> bool {
        matches!(self, Self::HtPrefetchPair)
    }

    /// Whether the currently claimed prefix is sufficient to begin a batch.
    pub const fn ready(self, second_is_present: bool) -> bool {
        match self {
            Self::HtPrefetchPair => second_is_present,
            Self::HeStartSingleThenCheck => true,
        }
    }
}

/// Failure while preparing a referenced/cache-TX A-MPDU batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferencedHtAmpduError {
    Frame(StationFrameError),
    Tx(HtAmpduTxError),
    BatchFull,
    /// The reserved prefix could not leave an aligned S31 metadata word
    /// immediately before the encoded MPDU.
    DmaPrefixGeometry {
        encoded_offset: usize,
        metadata_size: usize,
    },
}

impl From<StationFrameError> for ReferencedHtAmpduError {
    fn from(value: StationFrameError) -> Self {
        Self::Frame(value)
    }
}

impl From<HtAmpduTxError> for ReferencedHtAmpduError {
    fn from(value: HtAmpduTxError) -> Self {
        Self::Tx(value)
    }
}

/// One referenced A-MPDU transaction that owns both descriptors and frames.
///
/// A successful [`Self::push_ht`] moves a [`PinnedTxFrame`] into this value
/// before any descriptor can be published. Consequently `embassy-net` cannot
/// reuse the allocation until completion, detach and release.
///
/// Dropping a merely prepared batch cancels it and returns the leases. Dropping
/// a hardware-owned or non-detached batch deliberately leaks its finite leases
/// instead of returning DMA-visible memory to `embassy-net`. This fail-closed
/// path preserves memory safety after an abandoned hardware transaction; the
/// normal completion/timeout methods release every slot.
pub struct ReferencedHtAmpduBatch<
    'storage,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
> {
    storage: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    frames: [Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>;
        SLOTS],
    cookie: TxCookie,
    count: usize,
}

impl<
    'storage,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
>
    ReferencedHtAmpduBatch<
        'storage,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        BUFFER_SIZE,
    >
{
    pub fn begin(
        mut storage: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    ) -> Result<Self, HtAmpduTxError> {
        let cookie = storage.as_mut().begin()?;
        Ok(Self {
            storage,
            frames: [const { None }; SLOTS],
            cookie,
            count: 0,
        })
    }

    pub const fn cookie(&self) -> TxCookie {
        self.cookie
    }

    pub fn state(&self) -> TxSlotState {
        self.storage.state()
    }

    /// Number of MPDUs in the currently prepared or completed aggregate.
    ///
    /// This follows the descriptor owner rather than the number of retained
    /// leases: after a partial BlockAck retry the batch deliberately keeps all
    /// original leases alive, while only the missing descriptor subset is
    /// republished.
    pub fn frame_count(&self) -> u8 {
        self.storage.frame_count()
    }

    pub fn held_frame_count(&self) -> usize {
        self.count
    }

    pub fn prepared_aggregate(&self) -> Result<HtAmpduLength, HtAmpduTxError> {
        self.storage.prepared_aggregate(self.cookie)
    }

    pub fn prepared_empty_delimiters(&self, index: u8) -> Result<u8, HtAmpduTxError> {
        self.storage.prepared_empty_delimiters(self.cookie, index)
    }

    pub fn can_push_ht(
        &self,
        ethernet_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<bool, HtAmpduTxError> {
        let frame_length = ethernet_length
            .checked_add(open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.storage.can_commit_referenced_ht_frame(
            self.cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            rate,
            HEADROOM + FRAME_CAPACITY + TRAILER,
        )
    }

    /// Check a network-owned Ethernet frame against the complete HE APEP,
    /// TXOP and pinned-allocation limits without consuming its sequence or PN.
    pub fn can_push_he(
        &self,
        ethernet_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<bool, HtAmpduTxError> {
        let frame_length = ethernet_length
            .checked_add(open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.storage.can_commit_referenced_he_frame_with_txop(
            self.cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            txop_limit,
            HEADROOM + FRAME_CAPACITY + TRAILER,
        )
    }

    pub fn can_push_ht_amsdu_pair(
        &self,
        first_ethernet_length: usize,
        second_ethernet_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<bool, ReferencedHtAmpduError> {
        let frame_length =
            sta_protected_amsdu_pair_frame_length(first_ethernet_length, second_ethernet_length)?;
        Ok(self.storage.can_commit_referenced_ht_frame(
            self.cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            rate,
            HEADROOM + FRAME_CAPACITY + TRAILER,
        )?)
    }

    /// Encode one network-owned Ethernet frame in place and append its
    /// referenced allocation to this HT A-MPDU.
    pub fn push_ht(
        &mut self,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        metadata: StaProtectedEthernetFrame,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<EncodedStaFrame, ReferencedHtAmpduError> {
        if self.count >= SLOTS {
            return Err(ReferencedHtAmpduError::BatchFull);
        }
        let ethernet_offset = frame.ethernet_offset();
        let ethernet_length = frame.ethernet_length();
        let encoded = metadata.encode_in_place(
            frame.storage_mut(),
            ethernet_offset,
            ethernet_length,
            DataHeControl::Disabled,
        )?;
        let Some(dma_offset) = encoded.offset.checked_sub(TX_AMPDU_METADATA_SIZE) else {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        };
        if dma_offset & 3 != 0 {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        }

        // SAFETY: `frame` moves into `self.frames` immediately after a
        // successful commit. This batch owns that lease through every exposed
        // hardware state transition and does not return it before the MAC
        // storage reaches Free.
        unsafe {
            self.storage.as_mut().commit_referenced_ht_frame(
                self.cookie,
                &mut frame.storage_mut()[dma_offset..],
                encoded.length,
                hardware_mic_length,
                empty_delimiters,
                rate,
            )?;
        }
        self.frames[self.count] = Some(frame);
        self.count += 1;
        Ok(encoded)
    }

    /// Encode one network-owned Ethernet frame in place and append its pinned
    /// allocation to an HE A-MPDU without a staging copy.
    pub fn push_he(
        &mut self,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        metadata: StaProtectedEthernetFrame,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<EncodedStaFrame, ReferencedHtAmpduError> {
        if self.count >= SLOTS {
            return Err(ReferencedHtAmpduError::BatchFull);
        }
        let ethernet_offset = frame.ethernet_offset();
        let ethernet_length = frame.ethernet_length();
        let encoded = metadata.encode_in_place(
            frame.storage_mut(),
            ethernet_offset,
            ethernet_length,
            DataHeControl::Disabled,
        )?;
        let Some(dma_offset) = encoded.offset.checked_sub(TX_AMPDU_METADATA_SIZE) else {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        };
        if dma_offset & 3 != 0 {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        }

        // SAFETY: this batch takes ownership of the pinned lease immediately
        // after commit and retains it through detach, BlockAck and retries.
        unsafe {
            self.storage.as_mut().commit_referenced_he_frame_with_txop(
                self.cookie,
                &mut frame.storage_mut()[dma_offset..],
                encoded.length,
                hardware_mic_length,
                rate,
                density,
                txop_limit,
            )?;
        }
        self.frames[self.count] = Some(frame);
        self.count += 1;
        Ok(encoded)
    }

    /// Copy two network-owned Ethernet MSDUs into one referenced A-MSDU MPDU.
    ///
    /// The first pinned allocation becomes the DMA backing and remains owned
    /// by this batch. The second lease is returned only after its complete
    /// Ethernet body has been copied into the first allocation.
    ///
    /// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
    /// ieee80211_encap_amsdu` grows the first cache ESF, copies each following
    /// ESF into it and recycles that source ESF immediately. This method owns
    /// the same copy/release edge without exposing a vendor layout or pointer.
    pub fn push_ht_amsdu_pair(
        &mut self,
        mut first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        second: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        metadata: StaProtectedEthernetFrame,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<EncodedStaFrame, ReferencedHtAmpduError> {
        if self.count >= SLOTS {
            return Err(ReferencedHtAmpduError::BatchFull);
        }
        let ethernet_offset = first.ethernet_offset();
        let ethernet_length = first.ethernet_length();
        let encoded = metadata.encode_amsdu_pair_in_place(
            first.storage_mut(),
            ethernet_offset,
            ethernet_length,
            second.ethernet(),
        )?;
        let Some(dma_offset) = encoded.offset.checked_sub(TX_AMPDU_METADATA_SIZE) else {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        };
        if dma_offset & 3 != 0 {
            return Err(ReferencedHtAmpduError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size: TX_AMPDU_METADATA_SIZE,
            });
        }

        // SAFETY: the first lease moves into `self.frames` immediately after
        // commit and remains there through all hardware states. The second
        // allocation is no longer referenced after the in-place encoder
        // returns, so dropping it now follows the vendor recycle edge.
        unsafe {
            self.storage.as_mut().commit_referenced_ht_frame(
                self.cookie,
                &mut first.storage_mut()[dma_offset..],
                encoded.length,
                hardware_mic_length,
                empty_delimiters,
                rate,
            )?;
        }
        self.frames[self.count] = Some(first);
        self.count += 1;
        drop(second);
        Ok(encoded)
    }

    pub fn submit<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        queue: LegacyTxQueue,
        config: HtAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        self.storage
            .as_mut()
            .submit(hardware, self.cookie, queue, config)
    }

    pub fn submit_he<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        self.storage
            .as_mut()
            .submit_he(hardware, self.cookie, queue, config)
    }

    pub fn acknowledge_completion<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<HtAmpduTxCompletion>, HtAmpduTxError> {
        self.storage.as_mut().acknowledge_completion(hardware)
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<bool, HtAmpduTxError> {
        self.storage
            .as_mut()
            .begin_timeout_abort(hardware, self.cookie)
    }

    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), HtAmpduTxError> {
        self.storage
            .as_mut()
            .finish_timeout_abort(hardware, self.cookie)
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), HtAmpduTxError> {
        self.storage
            .as_mut()
            .detach_completed(hardware, self.cookie)
    }

    pub fn retain_for_ampdu_retry(
        &mut self,
        retry_mask: u32,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        self.storage
            .as_mut()
            .retain_for_ampdu_retry(self.cookie, retry_mask)
    }

    pub fn completed_frame(&self, index: u8) -> Result<(&[u8], u8), HtAmpduTxError> {
        self.storage.completed_frame(self.cookie, index)
    }

    /// Release a detached completed batch and return every network slot.
    pub fn release_completed(mut self) -> Result<(), HtAmpduTxError> {
        self.storage.as_mut().release_completed(self.cookie)?;
        self.release_frames();
        Ok(())
    }

    fn release_frames(&mut self) {
        for frame in &mut self.frames[..self.count] {
            drop(frame.take());
        }
        self.count = 0;
    }

    fn forget_frames(&mut self) {
        for frame in &mut self.frames[..self.count] {
            if let Some(frame) = frame.take() {
                mem::forget(frame);
            }
        }
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use core::task::{Context, Waker};

    use open_esp_radio_embassy_net::{
        Driver as _, NoopRawMutex, PinnedResources, PinnedTxPool, TxToken as _,
    };
    use open_esp_radio_esp32s31_wifi_lmac::tx::{
        HtChannelWidth, HtGuardInterval, HtMcs, LegacyRate,
    };
    use open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;

    use super::*;

    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = TX_AMPDU_METADATA_SIZE + STA_PROTECTED_QOS_ETHERNET_HEADROOM;
    const TRAILER: usize = 12;
    const QUEUE_DEPTH: usize = 2;
    const FRAME_LENGTH: usize = 17;
    const HT_RATE: HtRate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );

    #[test]
    fn ingress_policy_claims_ht_pair_but_leaves_he_tail_for_capacity_check() {
        let ht = ReferencedAmpduIngressPolicy::for_rate(TxPhyRate::Ht(HT_RATE)).expect("HT policy");
        assert!(ht.prefetch_second());
        assert!(!ht.ready(false));
        assert!(ht.ready(true));

        let he = ReferencedAmpduIngressPolicy::for_rate(TxPhyRate::He(HeRate::bcc_dcm(
            open_esp_radio_esp32s31_wifi_lmac::tx::HeBccDcmMcs::Mcs0,
            open_esp_radio_esp32s31_wifi_lmac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        )))
        .expect("HE policy");
        assert!(!he.prefetch_second());
        assert!(he.ready(false));
        assert!(he.ready(true));

        assert_eq!(
            ReferencedAmpduIngressPolicy::for_rate(TxPhyRate::Legacy(LegacyRate::Ofdm54M)),
            None
        );
    }

    fn context() -> Context<'static> {
        Context::from_waker(Waker::noop())
    }

    fn send_test_frame(
        device: &mut open_esp_radio_embassy_net::PinnedDevice<
            '_,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
        marker: u8,
    ) {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(FRAME_LENGTH, |frame| {
                frame[..6].copy_from_slice(&[0x20, 0x21, 0x22, 0x23, 0x24, marker]);
                frame[6..12].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].copy_from_slice(&[marker; 3]);
            });
    }

    #[test]
    fn prepared_batch_retains_slots_and_drop_cancels_before_returning_them() {
        type NetworkResources =
            PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        type NetworkPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        let resources = std::boxed::Box::leak(std::boxed::Box::new(NetworkResources::new()));
        let pool = NetworkPool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
            NetworkPool::new(),
        )));
        let (mut device, radio) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        send_test_frame(&mut device, 1);
        send_test_frame(&mut device, 2);

        let mut tx_storage = core::pin::pin!(HtAmpduTxStorage::<2, 0>::new());
        let mut batch = ReferencedHtAmpduBatch::begin(tx_storage.as_mut()).unwrap();
        for sequence_number in [10, 11] {
            let frame = radio.try_receive_tx().unwrap();
            let encoded = batch
                .push_ht(
                    frame,
                    StaProtectedEthernetFrame {
                        bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                        sequence_number,
                        user_priority: 0,
                        peer_qos: true,
                        ccmp_header: [sequence_number as u8, 0, 0, 0x20, 0, 0, 0, 0],
                    },
                    8,
                    0,
                    HT_RATE,
                )
                .unwrap();
            assert_eq!(encoded.offset, TX_AMPDU_METADATA_SIZE);
            assert_eq!(encoded.length, 45);
        }
        assert_eq!(batch.frame_count(), 2);
        assert_eq!(batch.held_frame_count(), 2);
        assert_eq!(batch.prepared_aggregate().unwrap().subframes, 2);
        assert!(device.transmit(&mut context()).is_none());

        drop(batch);
        assert_eq!(tx_storage.as_ref().state(), TxSlotState::Free);
        assert!(device.transmit(&mut context()).is_some());
    }

    #[test]
    fn descriptor_only_batch_encodes_he_delimiters_in_the_pinned_network_frame() {
        type NetworkResources =
            PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        type NetworkPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        let resources = std::boxed::Box::leak(std::boxed::Box::new(NetworkResources::new()));
        let pool = NetworkPool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
            NetworkPool::new(),
        )));
        let (mut device, radio) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        send_test_frame(&mut device, 1);

        let frame = radio.try_receive_tx().unwrap();
        let mut tx_storage = core::pin::pin!(HtAmpduTxStorage::<2, 0>::new());
        let mut batch = ReferencedHtAmpduBatch::begin(tx_storage.as_mut()).unwrap();
        let rate = HeRate::bcc_dcm(
            open_esp_radio_esp32s31_wifi_lmac::tx::HeBccDcmMcs::Mcs0,
            open_esp_radio_esp32s31_wifi_lmac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
        );
        assert!(
            batch
                .can_push_he(
                    frame.len(),
                    8,
                    rate,
                    HtAmpduDensity::SixteenMicroseconds,
                    HeEdcaTxopLimit::DEFAULT,
                )
                .unwrap()
        );
        let encoded = batch
            .push_he(
                frame,
                StaProtectedEthernetFrame {
                    bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                    sequence_number: 10,
                    user_priority: 0,
                    peer_qos: true,
                    ccmp_header: [10, 0, 0, 0x20, 0, 0, 0, 0],
                },
                8,
                rate,
                HtAmpduDensity::SixteenMicroseconds,
                HeEdcaTxopLimit::DEFAULT,
            )
            .unwrap();
        assert_eq!(batch.frame_count(), 1);
        let psdu_length = u16::try_from(encoded.length + 8 + 4).unwrap();
        let expected_empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, HtAmpduDensity::SixteenMicroseconds)
            .unwrap();
        assert_eq!(
            batch.prepared_empty_delimiters(0).unwrap(),
            expected_empty_delimiters
        );
        assert_eq!(batch.prepared_aggregate().unwrap().subframes, 1);
        drop(batch);
        assert_eq!(tx_storage.as_ref().state(), TxSlotState::Free);
    }

    #[test]
    fn amsdu_pair_recycles_second_slot_but_retains_first_until_batch_release() {
        type NetworkResources =
            PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        type NetworkPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        let resources = std::boxed::Box::leak(std::boxed::Box::new(NetworkResources::new()));
        let pool = NetworkPool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
            NetworkPool::new(),
        )));
        let (mut device, radio) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        send_test_frame(&mut device, 1);
        send_test_frame(&mut device, 2);

        let first = radio.try_receive_tx().unwrap();
        let second = radio.try_receive_tx().unwrap();
        let mut tx_storage = core::pin::pin!(HtAmpduTxStorage::<2, 0>::new());
        let mut batch = ReferencedHtAmpduBatch::begin(tx_storage.as_mut()).unwrap();
        let encoded = batch
            .push_ht_amsdu_pair(
                first,
                second,
                StaProtectedEthernetFrame {
                    bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                    sequence_number: 10,
                    user_priority: 0,
                    peer_qos: true,
                    ccmp_header: [10, 0, 0, 0x20, 0, 0, 0, 0],
                },
                8,
                0,
                HT_RATE,
            )
            .unwrap();
        assert_eq!(encoded.offset, TX_AMPDU_METADATA_SIZE);
        assert_eq!(batch.frame_count(), 1);
        assert_eq!(batch.held_frame_count(), 1);

        // The copied second source follows the vendor recycle edge and is
        // immediately available to the network stack. The first allocation
        // remains pinned because its bytes back the prepared DMA descriptor.
        let recycled_second = device.transmit(&mut context()).unwrap();
        recycled_second.consume(FRAME_LENGTH, |frame| frame.fill(3));
        assert!(device.transmit(&mut context()).is_none());
        let recycled_second = radio.try_receive_tx().unwrap();
        drop(recycled_second);

        drop(batch);
        assert_eq!(tx_storage.as_ref().state(), TxSlotState::Free);
        send_test_frame(&mut device, 3);
        send_test_frame(&mut device, 4);
        assert!(radio.try_receive_tx().is_some());
        assert!(radio.try_receive_tx().is_some());
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
> Drop
    for ReferencedHtAmpduBatch<
        '_,
        '_,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        BUFFER_SIZE,
    >
{
    fn drop(&mut self) {
        match self.storage.state() {
            TxSlotState::Free => self.release_frames(),
            TxSlotState::Reserved => {
                if self.storage.as_mut().cancel(self.cookie).is_ok() {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::Completed => {
                if self.storage.as_mut().release_completed(self.cookie).is_ok() {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::HardwareOwned | TxSlotState::ResetRequired => self.forget_frames(),
        }
    }
}
