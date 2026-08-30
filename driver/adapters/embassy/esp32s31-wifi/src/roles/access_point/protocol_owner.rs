const AP_PENDING_BUFFERED_RELEASE_CAPACITY: usize = 15;

struct PendingApBufferedReleases {
    slots: [Option<ApBufferedUnicastRelease>; AP_PENDING_BUFFERED_RELEASE_CAPACITY],
    read: usize,
    write: usize,
    len: usize,
}

impl PendingApBufferedReleases {
    const fn new() -> Self {
        Self {
            slots: [const { None }; AP_PENDING_BUFFERED_RELEASE_CAPACITY],
            read: 0,
            write: 0,
            len: 0,
        }
    }

    fn push(&mut self, release: ApBufferedUnicastRelease) -> Result<(), ApBufferedUnicastRelease> {
        if self.len == AP_PENDING_BUFFERED_RELEASE_CAPACITY {
            return Err(release);
        }
        debug_assert!(self.slots[self.write].is_none());
        self.slots[self.write] = Some(release);
        self.write = (self.write + 1) % AP_PENDING_BUFFERED_RELEASE_CAPACITY;
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<ApBufferedUnicastRelease> {
        if self.len == 0 {
            return None;
        }
        let release = self.slots[self.read]
            .take()
            .expect("non-empty AP release queue owns its read slot");
        self.read = (self.read + 1) % AP_PENDING_BUFFERED_RELEASE_CAPACITY;
        self.len -= 1;
        Some(release)
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// AP protocol state whose lifetime is independent of the shared physical TX
/// capability. Active and parked AP owners carry this exact value unchanged.
#[doc(hidden)]
pub struct Esp32s31AccessPointProtocolState<'storage, const DMA_BUFFER_SIZE: usize> {
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
    rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    rx_reorder_storage:
        &'storage RxReorderFrameStorage<DMA_BUFFER_SIZE, RX_REORDER_BACKING_SLOT_COUNT>,
    rx_addba_in_flight: Option<RxBlockAckActivation>,
    protocol_actions: Esp32s31AccessPointProtocolMailbox<AP_PROTOCOL_ACTION_CAPACITY>,
    pending_buffered_releases: PendingApBufferedReleases,
    /// Exact caller-owned group prefix advertised by the last successfully
    /// published DTIM beacon. Only the network-TX owner may consume it.
    pending_dtim_group_frames: Option<u16>,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    serviced_rx_frames: u64,
    serviced_rx_descriptors: u64,
    /// Terminal success bit for the most recently completed ordinary TX.
    /// The network owner consumes it while it owns a buffered release. For a
    /// group MPDU this is publication success only; no ACK exists.
    last_terminal_tx_succeeded: Option<bool>,
    #[cfg(feature = "diagnostics")]
    observer: &'static mut AccessPointObservationStorage,
    #[cfg(all(test, not(feature = "diagnostics")))]
    observer: AccessPointObservationStorage,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_observer: Option<&'static dyn AccessPointTerminalObserver>,
}

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
    state: Esp32s31AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>,
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
    state: Esp32s31AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>,
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    core::ops::Deref
    for Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
{
    type Target = Esp32s31AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    core::ops::DerefMut
    for Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize> core::ops::Deref
    for Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    type Target = Esp32s31AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize> core::ops::DerefMut
    for Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl<'storage, 'beacon, const DMA_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>
{
    pub const fn rx_batch_pending(&self) -> bool {
        self.state.rx_batch_offset < self.state.rx_batch_used
    }

    fn rx_batch_record(
        &self,
    ) -> Result<
        Option<crate::datapath::rx::ethernet::PackedEthernetRecord<'_>>,
        Esp32s31AccessPointControlError,
    > {
        crate::datapath::rx::ethernet::record_at(
            self.state.rx_frame,
            self.state.rx_batch_used,
            self.state.rx_batch_offset,
        )
        .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.state.rx_batch_offset);
        debug_assert!(next_offset <= self.state.rx_batch_used);
        self.state.rx_batch_offset = next_offset;
        if self.state.rx_batch_offset == self.state.rx_batch_used {
            self.state.rx_batch_offset = 0;
            self.state.rx_batch_used = 0;
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
            .chain(self.state.rx_reorder.next_deadline())
            .fold(beacon_deadline, u64::min))
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.mac.smallest_operational_tx_block_ack_window()
    }
}
