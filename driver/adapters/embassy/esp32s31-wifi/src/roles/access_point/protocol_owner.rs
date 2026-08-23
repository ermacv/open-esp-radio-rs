/// Control-plane owner for one active AP role.
pub struct Esp32s31AccessPointProtocolProcessor<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    serviced_rx_frames: u64,
    serviced_rx_descriptors: u64,
    #[cfg(feature = "diagnostics")]
    observer: &'static mut AccessPointObservationStorage,
    #[cfg(all(test, not(feature = "diagnostics")))]
    observer: AccessPointObservationStorage,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_observer: Option<&'static dyn AccessPointTerminalObserver>,
}

/// AP protocol state with the unique ordinary-TX resource removed.
///
/// This value retains peer, beacon, BlockAck, reorder, mailbox and report
/// state, but it cannot publish hardware until the paired physical owner
/// returns the exact ordinary-TX capability through `resume`.
pub struct Esp32s31AccessPointProtocolProcessorParked<
    'storage,
    'beacon,
    const DMA_BUFFER_SIZE: usize,
> {
    mac: Esp32s31ApMacParked<'beacon>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    serviced_rx_frames: u64,
    serviced_rx_descriptors: u64,
    #[cfg(feature = "diagnostics")]
    observer: &'static mut AccessPointObservationStorage,
    #[cfg(all(test, not(feature = "diagnostics")))]
    observer: AccessPointObservationStorage,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_observer: Option<&'static dyn AccessPointTerminalObserver>,
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    fn rx_batch_record(
        &self,
    ) -> Result<
        Option<crate::datapath::rx::ethernet::PackedEthernetRecord<'_>>,
        Esp32s31AccessPointControlError,
    > {
        crate::datapath::rx::ethernet::record_at(
            self.rx_frame,
            self.rx_batch_used,
            self.rx_batch_offset,
        )
        .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.rx_batch_offset);
        debug_assert!(next_offset <= self.rx_batch_used);
        self.rx_batch_offset = next_offset;
        if self.rx_batch_offset == self.rx_batch_used {
            self.rx_batch_offset = 0;
            self.rx_batch_used = 0;
        }
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.mac.beacon_publication_due(now_micros)
    }

    fn next_control_deadline_micros(
        &self,
        now_micros: u64,
    ) -> Result<u64, Esp32s31AccessPointControlError> {
        let (beacon_tick, _) = self
            .mac
            .next_beacon_delay(now_micros as u32)
            .ok_or(Esp32s31AccessPointControlError::InvalidBeaconSchedule)?;
        let beacon_deadline = now_micros
            .saturating_add(u64::from(beacon_tick.wrapping_sub(now_micros as u32)));
        Ok(self
            .mac
            .next_control_deadline()
            .into_iter()
            .chain(self.rx_reorder.next_deadline())
            .fold(beacon_deadline, u64::min))
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.mac.smallest_operational_tx_block_ack_window()
    }
}
