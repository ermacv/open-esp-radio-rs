#![forbid(unsafe_code)]

use core::pin::Pin;

use embassy_time::Instant;
use open_esp_radio::{
    esp32s31::{
        hal::RadioRegisters,
        wifi::{
            dma::tx_storage::TxDmaStorage,
            mac::{init::activate_promiscuous_receive, tx::TxSlot},
        },
    },
    wifi::{
        ieee80211::scan::{ScanObservation, ScanTable},
        softmac::interface::{BoundVirtualInterface, VifRole},
        sta::request::StationDiscovery,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
    scan_port::EmbassyEsp32s31ScanTimer,
    scan_rx::Esp32s31ScanFrameObserver,
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        Esp32s31StationInitialScanExit, Esp32s31StationInitialScanFailures,
        Esp32s31StationInitialScanPhase,
        Esp32s31StationInitialScanReturned,
        Esp32s31StationScanDecision, Esp32s31StationScanPlan, Esp32s31StationScanResources,
        Esp32s31StationScanReturned, complete_esp32s31_station_initial_scan,
        run_esp32s31_station_scan,
    },
};

use crate::{
    console::emergency_log,
    radio_hil::{
        ControlTxConfig, ETHERNET_FRAME, EmbassyWifiTxTimer, HilPhyObserver,
        OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM, OPEN_RADIO_RX_BUFFER_ADDRESSES,
        OPEN_RADIO_RX_DMA_STORAGE, OPEN_RADIO_TX_DMA_STORAGE, OPEN_RADIO_TX_SLOT_STORAGE,
        LISTEN_CHANNEL, OPEN_RADIO_TX_STATE, RX_DESCRIPTOR_COUNT, RadioHilConnectedTaskFixture,
        RadioHilJoinRx,
        RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStaNetwork,
        RadioHilStationPhase, RxStorage, SCAN_FRAME, SCAN_TABLE, ScanRx, TX_COMPLETION_DEADLINE_MS,
        TxStorage,
        UNICAST_TX_ATTEMPT_LIMIT, open_radio_tx_entropy,
    },
};

/// Cold resources prepared before the station task starts.
///
/// No scan or candidate selection occurs here. The returned RX owner is
/// halted, and the station actor becomes the sole owner of the first scan.
pub(in crate::radio_hil) struct RadioHilInitialScanResources {
    pub station_address: [u8; 6],
    pub mmio: RadioRegisters,
    pub rx: ScanRx,
    pub rx_storage: &'static RxStorage,
    pub tx_storage: &'static mut TxStorage,
    pub descriptor_base: u32,
    pub buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    pub scan_table: &'static mut ScanTable,
    pub scan_frame: &'static mut [u8],
    pub ethernet_frame: &'static mut [u8],
}

struct RadioHilInitialScanFrameObserver<'a> {
    station_address: [u8; 6],
    probe_responses: &'a mut u32,
}

impl Esp32s31ScanFrameObserver for RadioHilInitialScanFrameObserver<'_> {
    fn observe(&mut self, frame: &[u8], _rssi: i8, table_outcome: ScanObservation) {
        if frame.len() >= 16 && frame[0] & 0xfc == 0x50 && frame[4..10] == self.station_address {
            *self.probe_responses = self.probe_responses.saturating_add(1);
            if *self.probe_responses <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=addressed-probe-response \
                     count={} da={:02x?} sa={:02x?} table={table_outcome:?}",
                    *self.probe_responses,
                    &frame[4..10],
                    &frame[10..16],
                ));
            }
        }
    }
}

pub(in crate::radio_hil) fn prepare_initial_station_scan(
    state: &mut open_esp_radio::esp32s31::phy::phy_cold::PhyColdState,
    mut mmio: RadioRegisters,
    station_interface: BoundVirtualInterface,
) -> Option<RadioHilInitialScanResources> {
    let storage = OPEN_RADIO_RX_DMA_STORAGE.take();
    let tx_dma = TxDmaStorage::pin_static(OPEN_RADIO_TX_DMA_STORAGE.take())
        .expect("TX DMA storage must be addressable by ESP32-S31");
    let tx_slot =
        Pin::static_mut(OPEN_RADIO_TX_SLOT_STORAGE.init_with(|| TxSlot::from_dma(tx_dma)));
    let tx_storage = OPEN_RADIO_TX_STATE.init_with(|| {
        TxStorage::from_slot(
            tx_slot,
            state
                .tx_target_power_profile()
                .with_maximum_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM),
            open_radio_tx_entropy as fn() -> u32,
            EmbassyWifiTxTimer,
            ControlTxConfig {
                unicast_attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
                completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
                poll_interval_us: 1,
            },
        )
    });
    let buffer_addresses = OPEN_RADIO_RX_BUFFER_ADDRESSES.take();
    let descriptor_base = storage
        .dma_layout(buffer_addresses)
        .expect("RX DMA storage must be addressable by ESP32-S31");
    let buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT] = buffer_addresses;

    if station_interface.interface.role != VifRole::Station {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=station-binding role={:?}",
            station_interface.interface.role,
        ));
        return None;
    }
    let station_address = station_interface.interface.address;
    activate_promiscuous_receive(&mut mmio);
    let rx = match ScanRx::prepare_initial(&mut mmio, storage, descriptor_base, buffer_addresses) {
        Ok(rx) => rx,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-stage error={error:?}"
            ));
            return None;
        }
    };
    let scan_table = SCAN_TABLE.init_with(ScanTable::new);
    scan_table.clear();
    let scan_frame = SCAN_FRAME.take();
    let ethernet_frame = ETHERNET_FRAME.take();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=rx-prepared descriptor_base={descriptor_base:#010x} \
         buffer0={:#010x}",
        buffer_addresses[0],
    ));
    Some(RadioHilInitialScanResources {
        station_address,
        mmio,
        rx,
        rx_storage: storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        ethernet_frame,
    })
}

pub(in crate::radio_hil) async fn run_initial_station_scan_attempt<'fixture, 'security>(
    phase: Esp32s31StationInitialScanPhase<
        'security,
        RadioHilConnectedTaskFixture<'fixture>,
        RadioRegisters,
        ScanRx,
        RadioHilStaNetwork,
    >,
    discovery: StationDiscovery,
    scan_qualified: &mut bool,
) -> Esp32s31StationInitialScanExit<
    'security,
    RadioHilConnectedTaskFixture<'fixture>,
    RadioRegisters,
    RadioHilJoinRx<'static>,
    RadioHilStaNetwork,
    RadioHilStaLifecycleOwner<'fixture, 'security>,
    RadioHilStaLifecycleFailure,
> {
    let (mut fixture, hardware, receive, network, identity, mut security) = phase.into_parts();
    let (radio_resources, storage_resources, _) = fixture.split_mut();
    let (state, platform, interrupt_epoch) = radio_resources.parts_mut();
    let (_, tx_storage, scan_table, scan_frame, _) = storage_resources.parts_mut();
    let control = tx_storage
        .take_control()
        .expect("initial scan owns the control TX owner");
    let interrupt_setup = interrupt_epoch
        .setup()
        .expect("initial scan requires a quiesced interrupt epoch");
    let mut addressed_probe_responses = 0;
    let scan_started = Instant::now();
    let scan_plan = Esp32s31StationScanPlan::new(discovery, Some(LISTEN_CHANNEL as u8));
    let scan_channel_count = usize::from(scan_plan.channel_count());
    let target_ssid = scan_plan.target_ssid();
    let scan = run_esp32s31_station_scan(
        Esp32s31StationScanResources {
            phy: state,
            platform,
            phy_observer: HilPhyObserver,
            phy_delay: EmbassyPhyDelay,
            hardware,
            receive,
            control,
            interrupt_setup,
            table: scan_table,
            frame: scan_frame,
            scan_observer: RadioHilInitialScanFrameObserver {
                station_address: identity.station_address,
                probe_responses: &mut addressed_probe_responses,
            },
            sequence: security.sequences.non_qos_mut(),
            timer: EmbassyEsp32s31ScanTimer,
        },
        scan_plan.request(identity.station_address),
    )
    .await;
    let decision = scan.decision;
    let Esp32s31StationScanReturned {
        hardware,
        receive,
        control,
        phy_observer: _,
        phy_delay: _,
        scan_observer: observer,
        timer: _,
        table,
        frame: _,
        sequence: _,
        telemetry,
        transmit,
    } = scan.returned;
    let probe_responses = *observer.probe_responses;
    tx_storage
        .restore_control(control)
        .unwrap_or_else(|_| panic!("initial scan returned over a live TX owner"));
    let summary = table.summary();
    let rx_dma_pass = summary.records != 0 && telemetry.raw_frames != 0;
    let active_scan_pass = transmit.completions >= scan_channel_count as u32
        && probe_responses != 0
        && transmit.failures == 0;
    *scan_qualified = rx_dma_pass && active_scan_pass;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-initial-scan-owner-return \
         elapsed_ms={} records={} raw_frames={} ring_epochs={} probe_completions={} \
         probe_failures={} probe_responses={} rx_pass={} active_pass={}",
        scan_started.elapsed().as_millis(),
        summary.records,
        telemetry.raw_frames,
        telemetry.ring_epochs,
        transmit.completions,
        transmit.failures,
        probe_responses,
        u8::from(rx_dma_pass),
        u8::from(active_scan_pass),
    ));

    match &decision {
        Esp32s31StationScanDecision::Selected {
            candidate,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=active-scan channels={} \
                 channels_completed={} bssid={:02x?} channel={} rssi={}",
                scan_channel_count,
                progress.channels_completed,
                candidate.bssid,
                candidate.channel,
                candidate.rssi,
            ));
        }
        Esp32s31StationScanDecision::NoCandidate { progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-target channels_completed={} \
                 ssid={target_ssid:?}",
                progress.channels_completed,
            ));
        }
        Esp32s31StationScanDecision::Stopped { progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=initial-scan-service-stop \
                 channels_completed={}",
                progress.channels_completed,
            ));
        }
        Esp32s31StationScanDecision::Failed { error, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=initial-scan-service \
                 channels_completed={} error={error:?}",
                progress.channels_completed,
            ));
        }
        Esp32s31StationScanDecision::InvalidPlan { error, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=initial-scan-plan \
                 channels_planned={} error={error:?}",
                progress.channels_planned,
            ));
        }
    };
    complete_esp32s31_station_initial_scan(
        Esp32s31StationInitialScanReturned {
            runtime: fixture,
            hardware,
            receive,
            network,
            identity,
            security,
        },
        decision,
        |receive| receive.into_halted().map(RadioHilJoinRx::from_halted),
        |fixture, hardware, receive, network, identity, security| {
            RadioHilStaLifecycleOwner::new(
                fixture,
                RadioHilStationPhase::InitialScan {
                    hardware,
                    receive,
                    network,
                    identity,
                },
                security.into_role(),
            )
        },
        Esp32s31StationInitialScanFailures {
            no_candidate: RadioHilStaLifecycleFailure::InitialScanNoCandidate,
            receive_handoff: RadioHilStaLifecycleFailure::InitialScanReceiveHandoff,
            transaction: RadioHilStaLifecycleFailure::InitialScanTransaction,
            invalid_plan: RadioHilStaLifecycleFailure::InitialScanPlan,
        },
    )
}
