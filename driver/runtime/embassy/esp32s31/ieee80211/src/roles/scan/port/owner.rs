#![expect(
    clippy::type_complexity,
    reason = "the scan owner result exposes every exact phase owner without type erasure"
)]

use super::*;
impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize>
    Esp32s31ScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
{
    pub const fn new(
        radio: Esp32s31ScanRadio<P, H, R, T>,
        storage: Esp32s31ScanStorage<'resources, 'sequence, O, RECORDS>,
        station: Esp32s31ScanStation<'ssid, 'rates>,
        timer: W,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
            timer,
            telemetry: Esp32s31ScanTelemetry {
                raw_frames: 0,
                ring_epochs: 0,
            },
        }
    }

    pub fn into_parts(
        self,
    ) -> Esp32s31ScanPortParts<'resources, 'sequence, P, H, R, T, W, O, RECORDS> {
        let Self {
            radio,
            storage,
            station: _,
            timer,
            telemetry,
        } = self;
        let Esp32s31ScanRadio {
            phy,
            hardware,
            rx,
            tx,
        } = radio;
        let Esp32s31ScanStorage {
            table,
            frame,
            observer,
            sequence,
        } = storage;
        Esp32s31ScanPortParts {
            phy,
            hardware,
            rx,
            tx,
            timer,
            observer,
            table,
            frame,
            sequence,
            telemetry,
        }
    }

    pub(super) fn observe_scan_rx(
        &mut self,
        channel: u8,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31ScanPortError<P::Error, R::Error, T::Error>>
    where
        P: Esp32s31ScanPhyPort<H>,
        R: Esp32s31ScanReceivePort<H>,
        T: Esp32s31ScanTransmitPort<H>,
        O: Esp32s31ScanFrameObserver,
    {
        let mut context = Esp32s31ScanObservationContext::new(
            channel,
            self.storage.frame,
            self.storage.table,
            &mut self.storage.observer,
        );
        let progress = self
            .radio
            .rx
            .observe_management(&mut self.radio.hardware, &mut context)
            .map_err(Esp32s31ScanPortError::Receive)?;
        self.telemetry.raw_frames = self
            .telemetry
            .raw_frames
            .saturating_add(progress.completed_descriptors);
        if progress.recycled_descriptors != 0 {
            self.telemetry.ring_epochs = self.telemetry.ring_epochs.saturating_add(1);
        }
        Ok(progress)
    }
}
