struct DeferredAccessPointRxSink<'storage> {
    frames: crate::datapath::rx::ethernet::PackedEthernetWriter<'storage>,
    exhausted: bool,
}

impl<'storage> DeferredAccessPointRxSink<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: crate::datapath::rx::ethernet::PackedEthernetWriter::new(storage),
            exhausted: false,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
    }
}

impl Esp32s31ApRxSink for DeferredAccessPointRxSink<'_> {
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx_sink")
    )]
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
        if self.frames.push(event.frame).is_err() {
            self.exhausted = true;
        }
    }
}

/// Captures one ordinary Ethernet view as offsets inside its staging owner.
/// The owner is converted in place only after the AP dispatcher and reorder
/// state have accepted the frame.
struct InPlaceAccessPointRxSink {
    raw_start: usize,
    raw_length: usize,
    publication: Option<StagedEthernetPublication>,
    unsupported: bool,
}

impl InPlaceAccessPointRxSink {
    fn new(raw: &[u8]) -> Self {
        Self {
            raw_start: raw.as_ptr() as usize,
            raw_length: raw.len(),
            publication: None,
            unsupported: false,
        }
    }
}

impl Esp32s31ApRxSink for InPlaceAccessPointRxSink {
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
        let payload_start = event.frame.payload.as_ptr() as usize;
        let payload_end = match payload_start.checked_add(event.frame.payload.len()) {
            Some(end) => end,
            None => {
                self.unsupported = true;
                return;
            }
        };
        let raw_end = self.raw_start.saturating_add(self.raw_length);
        if event.amsdu
            || self.publication.is_some()
            || payload_start < self.raw_start
            || payload_end > raw_end
        {
            self.unsupported = true;
            return;
        }
        self.publication = Some(StagedEthernetPublication {
            destination: event.frame.destination,
            source: event.frame.source,
            ether_type: event.frame.ether_type,
            payload_offset: payload_start - self.raw_start,
            payload_length: event.frame.payload.len(),
            metadata: event.metadata,
        });
    }
}

#[cfg(test)]
mod in_place_rx_sink_tests {
    use open_esp_radio_wifi_softmac::MacRxMetadata;

    use super::*;

    #[test]
    fn active_tx_protocol_consumer_has_no_hardware_capability() {
        assert!(rx_protocol_consumer_has_hardware(false));
        assert!(!rx_protocol_consumer_has_hardware(true));
    }

    fn event<'a>(payload: &'a [u8], amsdu: bool) -> Esp32s31ApRxEvent<'a> {
        Esp32s31ApRxEvent {
            frame: EthernetFrameParts {
                destination: [1, 2, 3, 4, 5, 6],
                source: [7, 8, 9, 10, 11, 12],
                ether_type: 0x0800,
                payload,
            },
            raw: payload,
            amsdu,
            metadata: MacRxMetadata::unavailable(),
        }
    }

    #[test]
    fn captures_one_ordinary_frame_as_staging_offsets() {
        let raw = [0_u8; 64];
        let mut sink = InPlaceAccessPointRxSink::new(&raw);

        sink.publish(event(&raw[17..43], false));

        let publication = sink.publication.expect("ordinary frame is captured");
        assert_eq!(publication.payload_offset, 17);
        assert_eq!(publication.payload_length, 26);
        assert!(!sink.unsupported);
    }

    #[test]
    fn rejects_amsdu_and_payloads_outside_the_staging_owner() {
        let raw = [0_u8; 64];
        let external = [0_u8; 8];

        let mut amsdu = InPlaceAccessPointRxSink::new(&raw);
        amsdu.publish(event(&raw[16..24], true));
        assert!(amsdu.publication.is_none());
        assert!(amsdu.unsupported);

        let mut outside = InPlaceAccessPointRxSink::new(&raw);
        outside.publish(event(&external, false));
        assert!(outside.publication.is_none());
        assert!(outside.unsupported);
    }

    #[test]
    fn current_frame_joins_an_older_deferred_reorder_release() {
        assert!(can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            false,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            false,
            64
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            true,
            true,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::SharedStaging,
            false,
            false,
            0
        ));
        assert!(!can_publish_ap_rx_in_place(
            AccessPointRxPublication::OwnedNetworkPool,
            true,
            false,
            0
        ));
    }

}

fn observe_protected_dispatch(
    dispatch: Esp32s31ApRxDispatch,
    peer: Option<[u8; 6]>,
    #[cfg(any(feature = "diagnostics", test))] report: &mut Esp32s31AccessPointControlObservation,
    activity_peer: &mut Option<[u8; 6]>,
) -> bool {
    match dispatch {
        Esp32s31ApRxDispatch::Data {
            ethernet_frames, ..
        } => {
            if ethernet_frames != 0 {
                *activity_peer = peer;
                true
            } else {
                false
            }
        }
        Esp32s31ApRxDispatch::FragmentBuffered { .. } => false,
        Esp32s31ApRxDispatch::Duplicate => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_duplicates =
                    report.protected_data_duplicates.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::ForeignPeer => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_foreign = report.protected_data_foreign.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Unauthorized => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_unauthorized =
                    report.protected_data_unauthorized.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_radio_rejected =
                    report.protected_data_radio_rejected.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_protocol_rejected =
                    report.protected_data_protocol_rejected.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::SecurityModeMismatch) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.security_mode_mismatches =
                    report.security_mode_mismatches.saturating_add(1);
            }
            false
        }
        Esp32s31ApRxDispatch::Rejected(
            Esp32s31ApRxError::PeerQosMismatch
            | Esp32s31ApRxError::PairwiseKeyId(_)
            | Esp32s31ApRxError::Replay(_)
            | Esp32s31ApRxError::KeyGenerationMismatch
            | Esp32s31ApRxError::Fragment(_),
        ) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_protocol_rejected =
                    report.protected_data_protocol_rejected.saturating_add(1);
            }
            false
        }
    }
}

/// Hot AP data-dispatch leaf shared by direct and retained reorder releases.
/// It owns no hardware capability and reports only value/borrowed protocol
/// outcomes to its caller.
struct AccessPointProtectedFrameDispatch;

impl AccessPointProtectedFrameDispatch {
    #[cfg_attr(
        all(target_arch = "riscv32", not(feature = "task-poll-telemetry")),
        unsafe(link_section = ".hot.text.open_radio_ap_rx_dispatch")
    )]
    #[inline(never)]
    fn dispatch(
        data_rx: &mut Esp32s31ApRxDispatcher,
        ordered: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>,
        mut admit: impl FnMut(Esp32s31ApRxAdmissionRequest) -> Esp32s31ApRxAdmission,
        peer: Option<[u8; 6]>,
        publication: AccessPointRxPublication,
        current_buffer: usize,
        current_is_amsdu: bool,
        now_micros: u64,
        deferred: &mut DeferredAccessPointRxSink<'_>,
        in_place: &mut InPlaceAccessPointRxSink,
        #[cfg(any(feature = "diagnostics", test))]
        report: &mut Esp32s31AccessPointControlObservation,
        activity_peer: &mut Option<[u8; 6]>,
        produced_data: &mut bool,
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx: &mut crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile,
    ) {
        #[cfg(feature = "task-poll-telemetry")]
        let phase_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        #[cfg(feature = "task-poll-telemetry")]
        let publish_check_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_peer(now.wrapping_sub(phase_started));
            now
        };
        let current = ordered.buffer.as_ptr() as usize == current_buffer;
        let current_can_publish_in_place = can_publish_ap_rx_in_place(
            publication,
            current,
            current_is_amsdu,
            deferred.used(),
        );
        let may_publish_in_place = data_rx.may_publish_in_place(ordered);
        #[cfg(feature = "task-poll-telemetry")]
        let body_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_publish_check(now.wrapping_sub(publish_check_started));
            now
        };
        #[cfg(feature = "task-poll-telemetry")]
        let mut admission_cycles = 0_u32;
        let mut measured_admit = |request| {
            #[cfg(feature = "task-poll-telemetry")]
            let started = crate::diagnostics::core0_rx_cycles::cycle_count();
            let outcome = admit(request);
            #[cfg(feature = "task-poll-telemetry")]
            {
                admission_cycles = admission_cycles.wrapping_add(
                    crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
                );
            }
            outcome
        };
        let outcome = if may_publish_in_place && current_can_publish_in_place {
            data_rx.dispatch_at(ordered, now_micros, &mut measured_admit, in_place)
        } else {
            data_rx.dispatch_at(ordered, now_micros, &mut measured_admit, deferred)
        };
        #[cfg(feature = "task-poll-telemetry")]
        let observe_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_body(now.wrapping_sub(body_started));
            core0_ap_rx.record_leaf_admission(admission_cycles);
            now
        };
        *produced_data |= observe_protected_dispatch(
            outcome,
            peer,
            #[cfg(any(feature = "diagnostics", test))]
            report,
            activity_peer,
        );
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.record_leaf_observe(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(observe_started),
        );
    }
}
