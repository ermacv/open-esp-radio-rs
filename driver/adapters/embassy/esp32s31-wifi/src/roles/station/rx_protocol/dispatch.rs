use super::*;

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> Esp32s31ConnectedRxProcessor<'queue, 'pool, 'scratch, 'irq, M, S, CAPACITY, SLOTS, REORDER_SLOTS>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub(super) async fn dispatch_retained_frame(
        &mut self,
        frame: RetainedRxFrame<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    ) -> ConnectedRxDispatch {
        match frame {
            RetainedRxFrame::Hot(frame) => self.dispatch_owned_frame(frame).await,
            RetainedRxFrame::Cold(frame) => self.dispatch_reordered_frame(frame).await,
        }
    }

    async fn dispatch_reordered_frame(
        &mut self,
        frame: RxReorderFrame<'pool, CAPACITY, REORDER_SLOTS>,
    ) -> ConnectedRxDispatch {
        let locked = frame.segment();
        let source = locked.as_segment();
        let ordinary = !self.runtime.dispatcher.may_publish_amsdu(source);
        let result = if ordinary {
            if let Some(scratch) = self.reorder_scratch.as_deref_mut() {
                let length = source.buffer.len();
                scratch[..length].copy_from_slice(source.buffer);
                let segment = open_esp_radio_esp32s31_wifi_mac::rx::RxSegment {
                    descriptor_address: source.descriptor_address,
                    descriptor_word0: source.descriptor_word0,
                    buffer: &scratch[..length],
                    next_descriptor_address: source.next_descriptor_address,
                };
                dispatch_non_amsdu_segment(
                    &mut self.runtime.dispatcher,
                    &mut self.sink,
                    self.mpdu,
                    segment,
                    None,
                    #[cfg(any(feature = "diagnostics", test))]
                    self.pipeline_observer,
                )
                .await
            } else {
                self.dispatch_segment(source, None).await
            }
        } else {
            self.dispatch_segment(source, None).await
        };
        drop(locked);
        drop(frame);
        result
    }

    pub(super) async fn dispatch_owned_frame(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        let runtime_received_at_micros = frame.runtime_received_at_micros();
        if self.runtime.dispatcher.may_publish_amsdu(frame.segment())
            || !self
                .runtime
                .dispatcher
                .may_publish_ethernet(frame.segment())
        {
            let result = self
                .dispatch_segment(frame.segment(), runtime_received_at_micros)
                .await;
            drop(frame);
            self.irq.notify_rx_capacity();
            return result;
        }

        #[cfg(any(feature = "diagnostics", test))]
        let wait_started = self.pipeline_observer.map(|observer| observer.now_micros());
        self.sink.wait_staged_ready().await;
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, wait_started) {
            observer.observe(RxPipelineObservation::NetworkReadyWait {
                micros: observer.elapsed_micros_since(started),
            });
        }

        #[cfg(any(feature = "diagnostics", test))]
        let dispatch_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let segment = frame.segment();
        let (result, ethernet) = {
            let mut capture = StagedEthernetCapture::new(&mut self.sink, segment.buffer);
            let result = self.runtime.dispatcher.dispatch_with_runtime_received_at(
                segment,
                self.mpdu,
                &mut [],
                runtime_received_at_micros,
                &mut capture,
            );
            (result, capture.captured)
        };
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, dispatch_started) {
            let (data, amsdu, amsdu_subframes) = match result {
                ConnectedRxDispatch::Data {
                    ethernet_frames,
                    amsdu,
                } => (true, amsdu, ethernet_frames),
                _ => (false, false, 0),
            };
            observer.observe(RxPipelineObservation::ProtocolDispatched {
                data,
                amsdu,
                amsdu_subframes,
                unit_bytes: segment.buffer.len(),
                micros: observer.elapsed_micros_since(started),
            });
        }

        let disposition = if let Some(ethernet) = ethernet {
            self.sink.publish_staged(frame, ethernet)
        } else {
            drop(frame);
            StagedRxDisposition::Released
        };
        if disposition == StagedRxDisposition::Released {
            self.irq.notify_rx_capacity();
        }
        result
    }

    async fn dispatch_segment(
        &mut self,
        segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
        runtime_received_at_micros: Option<u64>,
    ) -> ConnectedRxDispatch {
        if self.runtime.dispatcher.may_publish_amsdu(segment) {
            return self.dispatch_amsdu(segment).await;
        }
        dispatch_non_amsdu_segment(
            &mut self.runtime.dispatcher,
            &mut self.sink,
            self.mpdu,
            segment,
            runtime_received_at_micros,
            #[cfg(any(feature = "diagnostics", test))]
            self.pipeline_observer,
        )
        .await
    }

    async fn dispatch_amsdu(
        &mut self,
        segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
    ) -> ConnectedRxDispatch {
        #[cfg(any(feature = "diagnostics", test))]
        let dispatch_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let (result, used, metadata, power_save_delivery) = {
            let wants_power_save_delivery = self.sink.wants_power_save_delivery();
            let mut deferred =
                DeferredEthernetFrames::new(self.ethernet, wants_power_save_delivery);
            let result =
                self.runtime
                    .dispatcher
                    .dispatch(segment, self.mpdu, &mut [], &mut deferred);
            (
                result,
                deferred.used(),
                deferred.metadata,
                deferred.power_save_delivery,
            )
        };
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, dispatch_started) {
            let (data, amsdu, amsdu_subframes) = match result {
                ConnectedRxDispatch::Data {
                    ethernet_frames,
                    amsdu,
                } => (true, amsdu, ethernet_frames),
                _ => (false, false, 0),
            };
            observer.observe(RxPipelineObservation::ProtocolDispatched {
                data,
                amsdu,
                amsdu_subframes,
                unit_bytes: segment.buffer.len(),
                micros: observer.elapsed_micros_since(started),
            });
        }
        let raw = segment.buffer;
        let metadata = metadata.unwrap_or_else(MacRxMetadata::unavailable);
        if let Some(delivery) = power_save_delivery {
            self.sink
                .publish(ConnectedRxEvent::PowerSaveDelivery(delivery));
        }
        let mut offset = 0_usize;
        while let Some(record) =
            crate::datapath::rx::ethernet::record_at(self.ethernet, used, offset)
                .expect("the private A-MSDU writer produces valid records")
        {
            #[cfg(any(feature = "diagnostics", test))]
            let wait_started = self.pipeline_observer.map(|observer| observer.now_micros());
            self.sink.wait_ready().await;
            #[cfg(any(feature = "diagnostics", test))]
            if let (Some(observer), Some(started)) = (self.pipeline_observer, wait_started) {
                observer.observe(RxPipelineObservation::NetworkReadyWait {
                    micros: observer.elapsed_micros_since(started),
                });
            }
            self.sink.publish(ConnectedRxEvent::Ethernet {
                frame: record.frame,
                raw,
                amsdu: true,
                metadata,
            });
            offset = record.next_offset;
        }
        result
    }
}

struct StagedEthernetCapture<'sink, S> {
    sink: &'sink mut S,
    raw_start: usize,
    raw_length: usize,
    captured: Option<StagedEthernetPublication>,
}

impl<'sink, S> StagedEthernetCapture<'sink, S> {
    fn new(sink: &'sink mut S, raw: &[u8]) -> Self {
        Self {
            sink,
            raw_start: raw.as_ptr() as usize,
            raw_length: raw.len(),
            captured: None,
        }
    }
}

impl<S: ConnectedRxSink> ConnectedRxSink for StagedEthernetCapture<'_, S> {
    fn wants_power_save_delivery(&self) -> bool {
        self.sink.wants_power_save_delivery()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let ConnectedRxEvent::Ethernet {
            frame,
            amsdu: false,
            metadata,
            ..
        } = event
        else {
            self.sink.publish(event);
            return;
        };
        let payload_start = frame.payload.as_ptr() as usize;
        let payload_offset = payload_start
            .checked_sub(self.raw_start)
            .filter(|offset| {
                offset
                    .checked_add(frame.payload.len())
                    .is_some_and(|end| end <= self.raw_length)
            })
            .expect("ordinary staged Ethernet payload belongs to its raw frame");
        assert!(
            self.captured.is_none(),
            "one ordinary MPDU publishes exactly one Ethernet frame"
        );
        self.captured = Some(StagedEthernetPublication {
            destination: frame.destination,
            source: frame.source,
            ether_type: frame.ether_type,
            payload_offset,
            payload_length: frame.payload.len(),
            metadata,
        });
    }

    fn supports_esp_now_v2(&self) -> bool {
        self.sink.supports_esp_now_v2()
    }

    fn publish_esp_now_v2(
        &mut self,
        received: open_esp_radio_wifi_softmac::EspNowReceivedV2<'_>,
        metadata: open_esp_radio_wifi_softmac::MacRxMetadata<
            open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo,
        >,
    ) {
        self.sink.publish_esp_now_v2(received, metadata);
    }
}

async fn dispatch_non_amsdu_segment<
    const CAPACITY: usize,
    const SLOTS: usize,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
>(
    dispatcher: &mut ConnectedRxDispatcher,
    sink: &mut S,
    mpdu: &mut [u8],
    segment: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
    runtime_received_at_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))] pipeline_observer: Option<&dyn RxPipelineObserver>,
) -> ConnectedRxDispatch {
    if dispatcher.may_publish_ethernet(segment) || dispatcher.may_complete_fragment(segment) {
        // Keep the staging lease until the single-MSDU path owns its one
        // network output slot. A-MSDU uses the deferred streaming path
        // above and acquires one slot per decoded subframe.
        #[cfg(any(feature = "diagnostics", test))]
        let wait_started = pipeline_observer.map(|observer| observer.now_micros());
        sink.wait_ready().await;
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (pipeline_observer, wait_started) {
            observer.observe(RxPipelineObservation::NetworkReadyWait {
                micros: observer.elapsed_micros_since(started),
            });
        }
    }
    #[cfg(any(feature = "diagnostics", test))]
    let dispatch_started = pipeline_observer.map(|observer| observer.now_micros());
    let result = dispatcher.dispatch_with_runtime_received_at(
        segment,
        mpdu,
        &mut [],
        runtime_received_at_micros,
        sink,
    );
    #[cfg(any(feature = "diagnostics", test))]
    if let (Some(observer), Some(started)) = (pipeline_observer, dispatch_started) {
        let (data, amsdu, amsdu_subframes) = match result {
            ConnectedRxDispatch::Data {
                ethernet_frames,
                amsdu,
            } => (true, amsdu, ethernet_frames),
            _ => (false, false, 0),
        };
        observer.observe(RxPipelineObservation::ProtocolDispatched {
            data,
            amsdu,
            amsdu_subframes,
            unit_bytes: segment.buffer.len(),
            micros: observer.elapsed_micros_since(started),
        });
    }
    result
}
