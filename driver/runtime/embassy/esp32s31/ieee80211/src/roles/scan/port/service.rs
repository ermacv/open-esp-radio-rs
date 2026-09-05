#![expect(
    clippy::manual_async_fn,
    reason = "scan port implementations keep explicit borrowed Future contracts"
)]

use super::*;
use open_esp_radio_esp32s31_wifi_mac::init::{
    MacRuntimeStopHardware, MacSnifferHardware, activate_promiscuous_receive,
    deactivate_promiscuous_receive,
};
impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize>
    Esp32s31StaScanPort
    for Esp32s31ScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
where
    P: Esp32s31ScanPhyPort<H>,
    H: MacSnifferHardware + MacRuntimeStopHardware,
    R: Esp32s31ScanReceivePort<H>,
    T: Esp32s31ScanTransmitPort<H>,
    W: Esp32s31ScanTimer,
    O: Esp32s31ScanFrameObserver,
{
    type Channel = u8;
    type Candidate = ScanRecord;
    type Error = Esp32s31ScanPortError<P::Error, R::Error, T::Error>;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.storage.table.clear();
            self.telemetry = Esp32s31ScanTelemetry::default();
            self.radio.tx.begin_scan();
            self.radio
                .rx
                .prepare_initial(&mut self.radio.hardware)
                .map_err(Esp32s31ScanPortError::Receive)
        }
    }

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.radio
                .phy
                .switch_channel(&mut self.radio.hardware, context.channel)
                .await
                .map_err(Esp32s31ScanPortError::ChannelSwitch)
        }
    }

    fn start_receive(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            activate_promiscuous_receive(&mut self.radio.hardware);
            let started = self
                .radio
                .rx
                .start(&mut self.radio.hardware)
                .await
                .map_err(Esp32s31ScanPortError::Receive);
            if started.is_err() {
                deactivate_promiscuous_receive(&mut self.radio.hardware);
            } else {
                self.radio.hardware.resume_mac_runtime();
            }
            started
        }
    }

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_ {
        let request = Esp32s31ScanProbeRequest {
            source: self.station.station_address,
            sequence_number: self.storage.sequence.take(),
            ssid: b"",
            supported_rates: self.station.supported_rates,
            current_channel: Some(context.channel),
            descriptor_capacity: self.station.descriptor_capacity,
        };
        async move {
            self.radio
                .tx
                .transmit_probe_request(&mut self.radio.hardware, request)
                .await
                .map(Esp32s31ScanProbeReport::outcome)
                .map_err(Esp32s31ScanPortError::Transmit)
        }
    }

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.observe_scan_rx(context.channel).map(|_| ())
    }

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.timer.wait_dwell_tick().await;
            Ok(())
        }
    }

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            deactivate_promiscuous_receive(&mut self.radio.hardware);
            self.radio.hardware.request_mac_runtime_stop();
            Timer::after_micros(20).await;
            while self.radio.hardware.mac_runtime_active_state() != 0 {
                Timer::after_micros(1).await;
            }
            loop {
                let progress = self.observe_scan_rx(context.channel)?;
                if progress.completed_descriptors == 0 {
                    break;
                }
            }
            self.radio.rx.park().map_err(Esp32s31ScanPortError::Receive)
        }
    }

    fn prepare_next_ring(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.radio
            .rx
            .prepare_next_channel(&mut self.radio.hardware)
            .map_err(Esp32s31ScanPortError::Receive)
    }

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
        if !self.station.select_candidate {
            return Ok(None);
        }
        Ok(best_matching_ssid_and_security(
            self.storage.table.records(),
            self.station.target_ssid,
            self.station.security,
        )
        .copied())
    }
}
