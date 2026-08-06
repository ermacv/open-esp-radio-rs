#![forbid(unsafe_code)]

use embassy_time::{Instant, Timer};
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
        scan_port::{
            EmbassyEsp32s31ScanTimer, Esp32s31ScanPort, Esp32s31ScanPortParts, Esp32s31ScanRadio,
            Esp32s31ScanStation, Esp32s31ScanStorage,
        },
        scan_rx::Esp32s31ScanFrameObserver,
    },
    esp32s31::{
        phy::phy_cold::PhyColdState,
        registers::MacInterruptSetup,
        wifi::{
            lmac::scan::{ScanObservation, ScanRecord, ScanTable},
            sta::{
                channel::Esp32s31ScanPhy,
                scan::{Esp32s31StaScanBackend, Esp32s31StaScanConfig, Esp32s31StaScanError},
            },
        },
    },
    wifi::{
        ieee80211::station::StaSequenceCounter,
        sta::scan::{StaCandidateScanExit, StaCandidateScanService, StaScanPlanError},
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;

use super::super::{
    HilPhyObserver, PROBE_REQUEST_RATES, PROBE_TX_DESCRIPTOR_CAPACITY, RadioHilDisconnectedEpoch,
    RadioHilRunningScanPortError, RunningScanRx, RunningScanTx, SCAN_DWELL_MS, STA_SCAN_CHANNELS,
    TxStorage,
};
use super::reporting::{RadioHilStationEpochProgress, RadioHilStationEpochReporter};
use crate::console::emergency_log;

/// Borrowed board context for one running candidate scan.
pub(in crate::radio_hil) struct RadioHilRunningScanContext<'fixture, 'ssid> {
    pub state: &'fixture mut PhyColdState,
    pub platform: &'fixture mut EspHalRadioPeripheral,
    pub tx_storage: &'fixture mut TxStorage,
    pub interrupt_setup: &'fixture MacInterruptSetup,
    pub scan_table: &'fixture mut ScanTable,
    pub scan_frame: &'fixture mut [u8],
    pub station_address: [u8; 6],
    pub target_ssid: &'ssid [u8],
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
    context: RadioHilRunningScanContext<'_, '_>,
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
        target_ssid,
        sequence,
    } = context;
    let open_esp_radio::adapters::esp32s31::wifi_embassy::station_epoch::Esp32s31RunningScanEpochParts {
        retained,
        hardware,
        rx,
    } = epoch.into_running_scan_parts();
    let control = tx_storage
        .take_control()
        .expect("connected teardown returned the ordinary TX owner");
    let scan_owner = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(
            Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(state, platform, HilPhyObserver),
            hardware,
            RunningScanRx::from_stopped(rx),
            RunningScanTx::new(control, interrupt_setup),
        ),
        Esp32s31ScanStorage::new(
            scan_table,
            scan_frame,
            RadioHilRunningScanFrameObserver {
                station_address,
                probe_responses: 0,
            },
            sequence,
        ),
        Esp32s31ScanStation::new(station_address, target_ssid, &PROBE_REQUEST_RATES)
            .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY as u32),
        EmbassyEsp32s31ScanTimer,
    );
    let scan_config =
        Esp32s31StaScanConfig::new(SCAN_DWELL_MS).expect("fixed HIL scan dwell policy is nonzero");
    let scan_backend = Esp32s31StaScanBackend::new(scan_config);
    let mut scan_service = StaCandidateScanService::new(scan_backend);
    let scan_started = Instant::now();
    let (scan_owner, outcome) = match scan_service.run(scan_owner, &STA_SCAN_CHANNELS).await {
        StaCandidateScanExit::Selected {
            owner,
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
            (owner, Ok(candidate))
        }
        StaCandidateScanExit::NoCandidate { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-running-scan \
                 channels={} error=no-candidate",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::NoCandidate {
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::Stopped { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error=stopped",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::Stopped {
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error={error:?}",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::Transaction {
                    error,
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan-plan \
                 channels={} error={error:?}",
                progress.channels_planned,
            ));
            (owner, Err(RadioHilRunningScanFailure::InvalidPlan(error)))
        }
    };

    let Esp32s31ScanPortParts {
        phy,
        hardware,
        rx,
        tx,
        timer: _,
        observer,
        telemetry,
        ..
    } = scan_owner.into_parts();
    let probe_responses = observer.probe_responses;
    let (_state, _platform, _observer) = phy.into_parts();
    let rx = match rx.into_stopped() {
        Ok(rx) => rx,
        Err(rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-running-scan-return phase={:?}",
                rx.phase(),
            ));
            let _owners = (retained, hardware, rx, tx);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let (control, tx_summary) = tx.into_parts();
    tx_storage
        .restore_control(control)
        .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-running-scan-owner-return \
         descriptor_base={:#010x} queued_frames={} probe_completions={} probe_failures={} \
         raw_frames={} probe_responses={} ring_epochs={}",
        rx.ring().descriptor_base(),
        rx.queued_frames(),
        tx_summary.completions,
        tx_summary.failures,
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
