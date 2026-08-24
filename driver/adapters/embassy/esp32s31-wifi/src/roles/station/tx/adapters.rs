#![expect(
    clippy::manual_async_fn,
    reason = "TX adapters keep the executor-neutral borrowed Future contracts explicit"
)]

use super::*;

impl<
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedControlTx
    for Esp32s31ConnectedTx<
        '_,
        '_,
        '_,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.take_last_ordinary_outcome()
    }

    fn now_micros(&self) -> u64 {
        self.ordinary.now_micros()
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        self.ordinary.peek_qos_sequence(tid)
    }

    fn start_action<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_action(hardware, body, config)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_power_management_null<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary
            .start_power_management_null(hardware, power_management)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_beacon_probe<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_beacon_probe(hardware)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_protected_eapol<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_protected_eapol(hardware, payload)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn set_tx_block_ack_agreement(&mut self, tid: u8, agreement: Option<(u16, bool)>) {
        self.set_block_ack_agreement(tid, agreement);
    }

    fn publish_he_trigger_response<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        _hardware: &mut H,
        request: HeTriggerRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        // Recheck at the final owner: control validation and this handoff are
        // separate calls, and neither may extend an expired response window.
        if self.ordinary.now_micros() >= request.response_deadline_micros {
            return Err(ConnectedHeControlRuntimeRejection::MissedResponseWindow);
        }
        if self.he_trigger_based != Some(request.queue_policy) {
            return Err(ConnectedHeControlRuntimeRejection::QueuePolicyMismatch);
        }
        if request.queue_policy.tid().value() != DATA_TID {
            return Err(ConnectedHeControlRuntimeRejection::QueueTidMismatch);
        }
        if self.active() {
            return Err(ConnectedHeControlRuntimeRejection::TxOwnerBusy);
        }
        if self.standby_error.is_some() {
            return Err(ConnectedHeControlRuntimeRejection::PreparedQueueFaulted);
        }
        let Some(prepared) = self.standby_prepared.as_ref() else {
            return Err(ConnectedHeControlRuntimeRejection::PreparedQueueUnavailable);
        };
        if !matches!(self.config.rate, TxPhyRate::He(_)) {
            return Err(ConnectedHeControlRuntimeRejection::UnsupportedQueueFormat);
        }
        if open_esp_radio_esp32s31_hal::types::MacHeTbLinkReservation::for_queue(
            request.queue_policy.tid_limit(),
            LegacyTxQueue::BestEffort.hardware_index(),
            prepared.original_subframes,
        )
        .is_none()
        {
            return Err(ConnectedHeControlRuntimeRejection::UnsupportedQueueGeometry);
        }

        // This is the single future-oracle attachment point. The retained
        // standby arena still owns every network lease and DMA byte, while the
        // reviewed lower layer can validate MPLEN/BSR. What is not reviewed is
        // the HE-TB PHY-vector/doorbell transition which would bind the parsed
        // RU, GI/LTF, MCS and DATA_LENGTH. Do not publish the HE-SU formatter's
        // vector as a substitute.
        Err(ConnectedHeControlRuntimeRejection::TbPhyPublicationUnverified)
    }

    fn publish_he_ndpa_feedback<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        _hardware: &mut H,
        request: HeNdpaRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        if self.ordinary.now_micros() >= request.response_deadline_micros {
            return Err(ConnectedHeControlRuntimeRejection::MissedResponseWindow);
        }
        if self.active() {
            return Err(ConnectedHeControlRuntimeRejection::TxOwnerBusy);
        }
        Err(ConnectedHeControlRuntimeRejection::NdpaFeedbackPublicationUnverified)
    }
}

impl<
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedControlTimer
    for Esp32s31ConnectedTx<
        '_,
        '_,
        '_,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
{
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_until_micros(deadline_micros)
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> DatapathNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    H: HtAmpduHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Error = AggregateTxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move { self.start_network(hardware, frame, network) }
    }

    fn last_started_frame_count(&self) -> usize {
        self.active_network_frame_count()
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        Esp32s31ConnectedTx::wait_deadline(self)
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        Esp32s31ConnectedTx::service(self, hardware, wake)
    }

    fn has_prepared(&self) -> bool {
        self.has_prepared_network_tx()
    }

    fn preferred_batch_size(&self) -> usize {
        self.preferred_network_batch_size()
    }

    fn prepared_frame_count(&self) -> usize {
        self.prepared_network_frame_count()
    }

    fn start_prepared(
        &mut self,
        hardware: &mut H,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        self.start_prepared_network(hardware, network)
    }

    fn cancel_prepared(&mut self) -> Result<(), Self::Error> {
        self.cancel_prepared_network()
    }

    fn can_prepare(&self) -> bool {
        self.can_prepare_network_tx()
    }

    fn prepare<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
    {
        async move {
            self.prepare_network_standby(frame, network);
            Ok(())
        }
    }
}
