//! AP-owned network TX transaction.
//!
//! WDEV schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;

pub(super) struct Esp32s31AccessPointNetworkTx<
    'aggregate,
    'storage,
    B: 'storage,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
> {
    aggregate: &'aggregate mut Esp32s31AccessPointAmpdu<'storage, B, SLOTS, BUFFER_SIZE>,
    deadline_micros: Option<u64>,
}

impl<'aggregate, 'storage, B, const SLOTS: usize, const BUFFER_SIZE: usize>
    Esp32s31AccessPointNetworkTx<'aggregate, 'storage, B, SLOTS, BUFFER_SIZE>
where
    B: StableDmaBacking + 'storage,
{
    pub(super) const fn new(
        aggregate: &'aggregate mut Esp32s31AccessPointAmpdu<'storage, B, SLOTS, BUFFER_SIZE>,
    ) -> Self {
        Self {
            aggregate,
            deadline_micros: None,
        }
    }

    pub(super) const fn aggregate_pending(&self) -> bool {
        self.deadline_micros.is_some()
    }
}

impl<
    'aggregate,
    'storage,
    'resources: 'storage,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
>
    Esp32s31AccessPointNetworkTx<
        'aggregate,
        'storage,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        SLOTS,
        BUFFER_SIZE,
    >
where
    M: RawMutex,
{
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start<
        D,
        P,
        E,
        T,
        H,
        const COUNT: usize,
        const DMA_BUFFER_SIZE: usize,
        const DMA_STORAGE_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointControl<
            '_,
            '_,
            '_,
            D,
            P,
            E,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointWdevError>
    where
        D: Esp32s31RxFrontierDelay,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        let destination = frame
            .as_slice()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
        let agreement = destination
            .filter(|peer| peer[0] & 1 == 0)
            .and_then(|peer| {
                control
                    .mac
                    .engine()
                    .tx_block_ack_agreement(peer)
                    .map(|agreement| (peer, agreement))
            });

        if let Some((peer, agreement)) = agreement
            && network.queue_len() != 0
            && let Some(mut second) = network.try_receive()
        {
            let second_peer = second
                .as_slice()
                .get(..6)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
            if second_peer != Some(peer) {
                network.requeue(second);
                return control
                    .start_network_tx(hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointWdevError::Control);
            }

            let rate =
                control
                    .mac
                    .peer_ht_rate(peer)
                    .ok_or(Esp32s31AccessPointWdevError::Control(
                        Esp32s31AccessPointControlError::InvalidPeerHtRate,
                    ))?;
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            let first_offset = frame.ethernet_offset();
            let first_length = frame.ethernet_length();
            let first_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    peer,
                    frame.storage_mut(),
                    first_offset,
                    first_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(
                        error,
                    ))
                })?;
            let aggregate = self.aggregate.active_mut();
            aggregate
                .begin(
                    peer,
                    rate,
                    first_encoded.sequence_number,
                    first_encoded.hardware_key_selector,
                )
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            aggregate
                .push(peer, frame, first_encoded)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;

            let second_offset = second.ethernet_offset();
            let second_length = second.ethernet_length();
            let second_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    peer,
                    second.storage_mut(),
                    second_offset,
                    second_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(
                        error,
                    ))
                })?;
            aggregate
                .push(peer, second, second_encoded)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;

            let target = usize::from(agreement.window).min(SLOTS);
            let mut admitted = 2_usize;
            while admitted < target {
                let Some(mut next) = network.try_receive() else {
                    break;
                };
                let next_peer = next
                    .as_slice()
                    .get(..6)
                    .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
                if next_peer != Some(peer) {
                    network.requeue(next);
                    break;
                }
                let offset = next.ethernet_offset();
                let length = next.ethernet_length();
                let encoded = engine
                    .encode_aggregate_ethernet_in_place(peer, next.storage_mut(), offset, length)
                    .map_err(|error| {
                        Esp32s31AccessPointWdevError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?;
                aggregate
                    .push(peer, next, encoded)
                    .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                admitted += 1;
            }
            aggregate
                .publish(ordinary, hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            self.deadline_micros = Some(
                ordinary
                    .now_micros()
                    .saturating_add(ordinary.publication_timeout_micros()),
            );
            return Ok(WifiTxProgress::Pending);
        }

        control
            .start_network_tx(hardware, frame.as_slice())
            .map_err(Esp32s31AccessPointWdevError::Control)
    }

    pub(super) async fn wait_deadline<
        D,
        P,
        E,
        T,
        const COUNT: usize,
        const DMA_BUFFER_SIZE: usize,
        const DMA_STORAGE_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointControl<
            '_,
            '_,
            '_,
            D,
            P,
            E,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) where
        D: Esp32s31RxFrontierDelay,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(deadline) = self.deadline_micros {
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("aggregate publication leaves ordinary AP TX idle");
            ordinary.wait_until(deadline).await;
        } else {
            control.wait_tx_deadline().await;
        }
    }

    pub(super) async fn service<
        D,
        P,
        E,
        T,
        H,
        const COUNT: usize,
        const DMA_BUFFER_SIZE: usize,
        const DMA_STORAGE_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointControl<
            '_,
            '_,
            '_,
            D,
            P,
            E,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointWdevError>
    where
        D: Esp32s31RxFrontierDelay,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        if self.deadline_micros.is_none() {
            return control
                .service_tx(hardware, wake)
                .await
                .map_err(Esp32s31AccessPointWdevError::Control);
        }

        let events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        let tx_events = events & (MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION);
        if tx_events.count_ones() > 1 {
            return Err(Esp32s31AccessPointWdevError::Aggregate(
                Esp32s31ApAmpduError::ConflictingInterruptEvents(tx_events),
            ));
        }
        if tx_events == MAC_INT_COLLISION {
            if !self
                .aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?
            {
                return Err(Esp32s31AccessPointWdevError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            self.deadline_micros = None;
            return Ok(WifiTxProgress::Complete);
        }
        if tx_events == MAC_INT_TX_TIMEOUT || matches!(wake, WifiTxWake::Deadline) {
            if !self
                .aggregate
                .active_mut()
                .begin_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?
            {
                return Err(Esp32s31AccessPointWdevError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            ordinary.after_micros(16).await;
            self.aggregate
                .active_mut()
                .finish_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            self.deadline_micros = None;
            return Ok(WifiTxProgress::Complete);
        }

        let aggregate_progress = {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            self.aggregate
                .active_mut()
                .service_completion(ordinary, hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?
        };
        match aggregate_progress {
            Esp32s31ApAmpduProgress::Complete => {
                self.deadline_micros = None;
                Ok(WifiTxProgress::Complete)
            }
            Esp32s31ApAmpduProgress::Republished => {
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(
                        error,
                    ))
                })?;
                self.deadline_micros = Some(
                    ordinary
                        .now_micros()
                        .saturating_add(ordinary.publication_timeout_micros()),
                );
                Ok(WifiTxProgress::Pending)
            }
            Esp32s31ApAmpduProgress::Pending => {
                if tx_events == MAC_INT_TX_COMPLETE {
                    return Err(Esp32s31AccessPointWdevError::Aggregate(
                        Esp32s31ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }
}
