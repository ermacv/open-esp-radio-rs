use super::*;
use core::{
    future::{Future, ready},
    pin::pin,
    task::{Context, Poll},
};
use std::vec::Vec;

use open_esp_radio_ieee80211::scan::ScanObservation;
use open_esp_radio_wifi_sta::scan::{StaCandidateScanExit, StaCandidateScanService};

use open_esp_radio_esp32s31_wifi_sta::scan::{Esp32s31StaScanBackend, Esp32s31StaScanConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Prepare,
    Switch(u8),
    Start,
    Probe(u8),
    Observe,
    Stop,
}

#[derive(Default)]
struct Hardware {
    actions: Vec<Action>,
}

struct Phy(u32);

impl Esp32s31ScanPhyPort<Hardware> for Phy {
    type Error = ();

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        channel: u8,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        hardware.actions.push(Action::Switch(channel));
        ready(Ok(()))
    }
}

struct Receive(u32);

impl Esp32s31ScanReceivePort<Hardware> for Receive {
    type Error = ();

    fn prepare_initial(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Prepare);
        Ok(())
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        hardware.actions.push(Action::Start);
        ready(Ok(()))
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut Hardware,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver,
    {
        hardware.actions.push(Action::Observe);
        let mut beacon = [0_u8; 47];
        beacon[0] = 0x80;
        beacon[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        beacon[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
        beacon[42..45].copy_from_slice(&[3, 1, 11]);
        beacon[45..47].copy_from_slice(&[48, 0]);
        assert_ne!(
            context.observe_management_frame(&beacon, -30),
            ScanObservation::Ignored
        );
        Ok(Esp32s31ScanRxProgress {
            completed_descriptors: 1,
            parsed_management_frames: 1,
            recycled_descriptors: 1,
            ..Esp32s31ScanRxProgress::default()
        })
    }

    fn stop(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Stop);
        Ok(())
    }

    fn prepare_next(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Prepare);
        Ok(())
    }
}

struct Transmit(u32);

impl Esp32s31ScanTransmitPort<Hardware> for Transmit {
    type Error = ();

    fn begin_scan(&mut self) {}

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a {
        hardware
            .actions
            .push(Action::Probe(request.current_channel.unwrap()));
        ready(Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt))
    }
}

#[derive(Default)]
struct DwellTimer(u32);

impl Esp32s31ScanTimer for DwellTimer {
    fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_ {
        self.0 += 1;
        ready(())
    }
}

#[derive(Default)]
struct Observer(u32);

impl Esp32s31ScanFrameObserver for Observer {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _outcome: ScanObservation) {
        self.0 += 1;
    }
}

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

#[test]
fn concrete_port_returns_every_owner_after_selected_candidate() {
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 128];
    let mut sequence = StaSequenceCounter::new(7);
    let port = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(Phy(11), Hardware::default(), Receive(22), Transmit(33)),
        Esp32s31ScanStorage::new(&mut table, &mut frame, Observer::default(), &mut sequence),
        Esp32s31ScanStation::new([7; 6], b"test", &[0x82, 0x84]).with_descriptor_capacity(88),
        DwellTimer::default(),
    );
    let backend = Esp32s31StaScanBackend::new(Esp32s31StaScanConfig::new(2).unwrap());
    let mut service = StaCandidateScanService::new(backend);
    let exit = block_on(service.run(port, &[11]));
    let (owner, candidate) = match exit {
        StaCandidateScanExit::Selected {
            owner, candidate, ..
        } => (owner, candidate),
        StaCandidateScanExit::NoCandidate { .. } => {
            panic!("matching beacon must select one candidate")
        }
        StaCandidateScanExit::Failed { error, .. } => {
            panic!("running scan failed: {error:?}")
        }
        StaCandidateScanExit::Stopped { .. } => panic!("running scan stopped"),
        StaCandidateScanExit::InvalidPlan { error, .. } => {
            panic!("running scan plan failed: {error:?}")
        }
    };
    assert_eq!(candidate.channel, 11);

    let parts = owner.into_parts();
    assert_eq!(parts.phy.0, 11);
    assert_eq!(parts.rx.0, 22);
    assert_eq!(parts.tx.0, 33);
    assert_eq!(parts.timer.0, 2);
    assert_eq!(parts.observer.0, 3);
    assert_eq!(
        parts.telemetry,
        Esp32s31ScanTelemetry {
            raw_frames: 3,
            ring_epochs: 3,
        }
    );
    assert_eq!(
        parts.hardware.actions,
        [
            Action::Prepare,
            Action::Switch(11),
            Action::Start,
            Action::Probe(11),
            Action::Observe,
            Action::Observe,
            Action::Observe,
            Action::Stop,
        ]
    );
    assert_eq!(parts.sequence.peek(), 8);
    assert_eq!(parts.table.summary().records, 1);
    assert_eq!(parts.frame.len(), 128);
}
