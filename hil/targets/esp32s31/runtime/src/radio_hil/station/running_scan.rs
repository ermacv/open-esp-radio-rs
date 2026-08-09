#![forbid(unsafe_code)]

use embassy_time::{Instant, Timer};
use open_esp_radio::{
    esp32s31::{
        phy::phy_cold::PhyColdState,
        registers::MacInterruptSetup,
        wifi::sta::scan::Esp32s31StaScanError,
    },
    wifi::{
        ieee80211::{
            scan::{ScanObservation, ScanRecord, ScanTable},
            station::StaSequenceCounter,
        },
        sta::scan::StaScanPlanError,
        sta::request::StationDiscovery,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
    scan_port::EmbassyEsp32s31ScanTimer,
    scan_rx::Esp32s31ScanFrameObserver,
    station::{
        Esp32s31StationScanDecision, Esp32s31StationScanPlan, Esp32s31StationScanResources,
        Esp32s31StationScanReturned, run_esp32s31_station_scan,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;

use super::super::{
    HilPhyObserver, LISTEN_CHANNEL, RadioHilDisconnectedEpoch, RadioHilRunningScanPortError,
    RunningScanRx, TxStorage,
};
use super::reporting::{RadioHilStationEpochProgress, RadioHilStationEpochReporter};
use crate::console::emergency_log;

/// Borrowed board context for one running candidate scan.
pub(in crate::radio_hil) struct RadioHilRunningScanContext<'fixture> {
    pub state: &'fixture mut PhyColdState,
    pub platform: &'fixture mut EspHalRadioPeripheral,
    pub tx_storage: &'fixture mut TxStorage,
    pub interrupt_setup: &'fixture MacInterruptSetup,
    pub scan_table: &'fixture mut ScanTable,
    pub scan_frame: &'fixture mut [u8],
    pub station_address: [u8; 6],
    pub discovery: StationDiscovery,
    pub sequence: &'fixture mut StaSequenceCounter,
}

pub(in crate::radio_hil) struct RadioHilRunningScanReturn {
    pub disconnected: RadioHilDisconnectedEpoch,
    pub candidate: ScanRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilRunningScanFailure {
    NoCandidate {
        channels_completed: u16,
    },
    Stopped {
        channels_completed: u16,
    },
    Transaction {
        error: Esp32s31StaScanError<RadioHilRunningScanPortError>,
        channels_completed: u16,
    },
    InvalidPlan(StaScanPlanError),
}

pub(in crate::radio_hil) struct RadioHilRunningScanRecovery {
    pub disconnected: RadioHilDisconnectedEpoch,
    pub failure: RadioHilRunningScanFailure,
}

struct RadioHilRunningScanFrameObserver {
    station_address: [u8; 6],
    probe_responses: u32,
}

impl Esp32s31ScanFrameObserver for RadioHilRunningScanFrameObserver {
    fn observe(&mut self, frame: &[u8], _rssi: i8, table_outcome: ScanObservation) {
        if frame.len() >= 10 && frame[0] & 0xfc == 0x50 && frame[4..10] == self.station_address {
            self.probe_responses = self.probe_responses.saturating_add(1);
            if self.probe_responses <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=addressed-probe-response \
                     count={} da={:02x?} sa={:02x?} table={table_outcome:?}",
                    self.probe_responses,
                    &frame[4..10],
                    &frame[10..16],
                ));
            }
        }
    }
}

/// Prove that one disconnected owner can complete a finite multi-channel
/// running scan and return every resource without reinitializing static
/// storage.
pub(in crate::radio_hil) async fn qualify_disconnected_running_scan(
    epoch: RadioHilDisconnectedEpoch,
    context: RadioHilRunningScanContext<'_>,
    reporter: RadioHilStationEpochReporter,
) -> Result<RadioHilRunningScanReturn, RadioHilRunningScanRecovery> {
    let RadioHilRunningScanContext {
        state,
        platform,
        tx_storage,
        interrupt_setup,
        scan_table,
        scan_frame,
        station_address,
        discovery,
        sequence,
    } = context;
    let open_esp_radio_esp32s31_wifi_embassy::station_epoch::Esp32s31RunningScanEpochParts {
        retained,
        hardware,
        rx,
    } = epoch.into_running_scan_parts();
    let control = tx_storage
        .take_control()
        .expect("connected teardown returned the ordinary TX owner");
    let scan_plan = Esp32s31StationScanPlan::new(discovery, Some(LISTEN_CHANNEL as u8));
    let scan_started = Instant::now();
    let scan = run_esp32s31_station_scan(
        Esp32s31StationScanResources {
            phy: state,
            platform,
            phy_observer: HilPhyObserver,
            phy_delay: EmbassyPhyDelay,
            hardware,
            receive: RunningScanRx::from_stopped(rx),
            control,
            interrupt_setup,
            table: scan_table,
            frame: scan_frame,
            scan_observer: RadioHilRunningScanFrameObserver {
                station_address,
                probe_responses: 0,
            },
            sequence,
            timer: EmbassyEsp32s31ScanTimer,
        },
        scan_plan.request(station_address),
    )
    .await;
    let outcome = match scan.decision {
        Esp32s31StationScanDecision::Selected {
            candidate,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-running-scan \
                 channels={} elapsed_ms={} candidate_channel={} candidate_rssi={}",
                progress.channels_completed,
                scan_started.elapsed().as_millis(),
                candidate.channel,
                candidate.rssi,
            ));
            Ok(candidate)
        }
        Esp32s31StationScanDecision::NoCandidate { progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-running-scan \
                 channels={} error=no-candidate",
                progress.channels_completed,
            ));
            Err(RadioHilRunningScanFailure::NoCandidate {
                channels_completed: progress.channels_completed,
            })
        }
        Esp32s31StationScanDecision::Stopped { progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error=stopped",
                progress.channels_completed,
            ));
            Err(RadioHilRunningScanFailure::Stopped {
                channels_completed: progress.channels_completed,
            })
        }
        Esp32s31StationScanDecision::Failed { error, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error={error:?}",
                progress.channels_completed,
            ));
            Err(RadioHilRunningScanFailure::Transaction {
                error,
                channels_completed: progress.channels_completed,
            })
        }
        Esp32s31StationScanDecision::InvalidPlan { error, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan-plan \
                 channels={} error={error:?}",
                progress.channels_planned,
            ));
            Err(RadioHilRunningScanFailure::InvalidPlan(error))
        }
    };
    let Esp32s31StationScanReturned {
        hardware,
        receive,
        control,
        timer: _,
        phy_observer: _,
        phy_delay: _,
        scan_observer,
        table: _,
        frame: _,
        sequence: _,
        telemetry,
        transmit,
    } = scan.returned;
    let probe_responses = scan_observer.probe_responses;
    let rx = match receive.into_stopped() {
        Ok(rx) => rx,
        Err(rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-running-scan-return phase={:?}",
                rx.phase(),
            ));
            let _owners = (retained, hardware, rx, control);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    tx_storage
        .restore_control(control)
        .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-running-scan-owner-return \
         descriptor_base={:#010x} queued_frames={} probe_completions={} probe_failures={} \
         raw_frames={} probe_responses={} ring_epochs={}",
        rx.ring().descriptor_base(),
        rx.queued_frames(),
        transmit.completions,
        transmit.failures,
        telemetry.raw_frames,
        probe_responses,
        telemetry.ring_epochs,
    ));
    let disconnected = retained.restore(hardware, rx);
    match outcome {
        Ok(candidate) => {
            reporter.report(RadioHilStationEpochProgress::ScanOwnersReturned);
            Ok(RadioHilRunningScanReturn {
                disconnected,
                candidate,
            })
        }
        Err(failure) => Err(RadioHilRunningScanRecovery {
            disconnected,
            failure,
        }),
    }
}
