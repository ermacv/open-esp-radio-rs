#![forbid(unsafe_code)]

use core::pin::Pin;

use embassy_time::Instant;
use open_esp_radio::{
    esp32s31::{
        hal::ColdRadioRegisters,
        wifi::{
            dma::tx_storage::TxDmaStorage,
            mac::{init::activate_promiscuous_receive, tx::TxSlot},
            sta::{
                channel::Esp32s31ScanPhy,
                scan::{Esp32s31StaScanBackend, Esp32s31StaScanConfig},
            },
        },
    },
    wifi::{
        ieee80211::{
            scan::{ScanObservation, ScanRecord, ScanTable, best_matching_ssid},
            station::StaSequenceCounter,
        },
        softmac::interface::{BoundVirtualInterface, VifRole},
        sta::scan::{StaCandidateScanExit, StaCandidateScanService},
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
    scan_port::{
        EmbassyEsp32s31ScanTimer, Esp32s31ScanPort, Esp32s31ScanPortParts, Esp32s31ScanRadio,
        Esp32s31ScanStation, Esp32s31ScanStorage,
    },
    scan_rx::Esp32s31ScanFrameObserver,
    sta_tx_epoch::Esp32s31StaTxEpochExt,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_hil_protocol::NetworkCredentials;

use crate::{
    console::emergency_log,
    radio_hil::{
        ControlTxConfig, ETHERNET_FRAME, EmbassyWifiTxTimer, HilPhyObserver,
        OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM, OPEN_RADIO_RX_BUFFER_ADDRESSES,
        OPEN_RADIO_RX_DMA_STORAGE, OPEN_RADIO_TX_DMA_STORAGE, OPEN_RADIO_TX_SLOT_STORAGE,
        OPEN_RADIO_TX_STATE, PROBE_REQUEST_RATES, PROBE_TX_DESCRIPTOR_CAPACITY,
        RX_DESCRIPTOR_COUNT, RX_STAGE_CAPACITY, RadioHilJoinRx, RxStorage, SCAN_DWELL_MS,
        SCAN_FRAME, SCAN_TABLE, STA_SCAN_CHANNEL_COUNT, STA_SCAN_CHANNELS, ScanRx, ScanTx,
        TX_COMPLETION_DEADLINE_MS, TxStorage, UNICAST_TX_ATTEMPT_LIMIT, open_radio_tx_entropy,
    },
};

pub(in crate::radio_hil) struct RadioHilColdScanHandoff {
    pub station_address: [u8; 6],
    pub cold_mmio: ColdRadioRegisters,
    pub rx: RadioHilJoinRx<'static>,
    pub rx_storage: &'static RxStorage,
    pub tx_storage: &'static mut TxStorage,
    pub descriptor_base: u32,
    pub buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    pub scan_table: &'static mut ScanTable,
    pub scan_frame: &'static mut [u8],
    pub ethernet_frame: &'static mut [u8],
    pub target: Option<ScanRecord>,
    pub scan_qualified: bool,
}

struct RadioHilColdScanFrameObserver<'a> {
    station_address: [u8; 6],
    probe_responses: &'a mut u32,
}

impl Esp32s31ScanFrameObserver for RadioHilColdScanFrameObserver<'_> {
    fn observe(&mut self, frame: &[u8], _rssi: i8, table_outcome: ScanObservation) {
        if frame.len() >= 10 && frame[0] & 0xfc == 0x50 && frame[4..10] == self.station_address {
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

pub(in crate::radio_hil) async fn run_cold_station_scan(
    state: &mut open_esp_radio::esp32s31::phy::phy_cold::PhyColdState,
    platform: &mut EspHalRadioPeripheral,
    mut cold_mmio: ColdRadioRegisters,
    network_credentials: &NetworkCredentials,
    station_interface: BoundVirtualInterface,
) -> Option<RadioHilColdScanHandoff> {
    let storage = OPEN_RADIO_RX_DMA_STORAGE.init_with(RxStorage::new);
    let tx_dma = TxDmaStorage::pin_static(OPEN_RADIO_TX_DMA_STORAGE.init_with(TxDmaStorage::new))
        .expect("TX DMA storage must be addressable by ESP32-S31");
    let tx_slot = Pin::static_mut(OPEN_RADIO_TX_SLOT_STORAGE.init(TxSlot::from_dma(tx_dma)));
    let tx_storage = OPEN_RADIO_TX_STATE.init(TxStorage::from_slot(
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
    ));
    let buffer_addresses = OPEN_RADIO_RX_BUFFER_ADDRESSES.init([0; RX_DESCRIPTOR_COUNT]);
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
    activate_promiscuous_receive(&mut cold_mmio);
    let cold_interrupt_mask = cold_mmio.mac_interrupt_enable();
    cold_mmio.mask_and_clear_mac_interrupts(u32::MAX);
    let scan_rx =
        match ScanRx::prepare_initial(&mut cold_mmio, storage, descriptor_base, buffer_addresses) {
            Ok(rx) => rx,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-stage error={error:?}"
                ));
                return None;
            }
        };
    let scan_table = SCAN_TABLE.init(ScanTable::new());
    scan_table.clear();
    let scan_frame = SCAN_FRAME.init([0; RX_STAGE_CAPACITY]);
    let ethernet_frame = ETHERNET_FRAME.init([0; RX_STAGE_CAPACITY]);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=rx-active descriptor_base={descriptor_base:#010x} \
         buffer0={:#010x} cold_int_mask={cold_interrupt_mask:#010x}",
        buffer_addresses[0],
    ));

    let scan_started = Instant::now();
    let scan_tx = ScanTx::new(
        tx_storage
            .take_control()
            .expect("cold scan owns the initial control TX owner"),
    );
    let mut scan_sequence = StaSequenceCounter::new(1);
    let mut addressed_probe_responses = 0;
    let scan_owner = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(
            Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(state, platform, HilPhyObserver),
            cold_mmio,
            scan_rx,
            scan_tx,
        ),
        Esp32s31ScanStorage::new(
            scan_table,
            scan_frame,
            RadioHilColdScanFrameObserver {
                station_address,
                probe_responses: &mut addressed_probe_responses,
            },
            &mut scan_sequence,
        ),
        Esp32s31ScanStation::new(
            station_address,
            network_credentials.ssid(),
            &PROBE_REQUEST_RATES,
        )
        .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY as u32),
        EmbassyEsp32s31ScanTimer,
    );
    let scan_config =
        Esp32s31StaScanConfig::new(SCAN_DWELL_MS).expect("fixed HIL scan dwell policy is nonzero");
    let mut scan_service = StaCandidateScanService::new(Esp32s31StaScanBackend::new(scan_config));
    let (scan_owner, primary_target) = match scan_service.run(scan_owner, &STA_SCAN_CHANNELS).await
    {
        StaCandidateScanExit::Selected {
            owner, candidate, ..
        } => (owner, Some(candidate)),
        StaCandidateScanExit::NoCandidate { owner, .. } => (owner, None),
        StaCandidateScanExit::Stopped { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-service-stop \
                 channels_completed={}",
                progress.channels_completed,
            ));
            let _owner = owner;
            return None;
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-service \
                 channels_completed={} error={error:?}",
                progress.channels_completed,
            ));
            let _owner = owner;
            return None;
        }
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-plan \
                 channels_planned={} error={error:?}",
                progress.channels_planned,
            ));
            let _owner = owner;
            return None;
        }
    };
    let Esp32s31ScanPortParts {
        phy,
        hardware: cold_mmio,
        rx: scan_rx,
        tx: scan_tx,
        observer,
        table: scan_table,
        frame: scan_frame,
        telemetry,
        ..
    } = scan_owner.into_parts();
    let raw_frames = telemetry.raw_frames;
    let ring_epochs = telemetry.ring_epochs;
    let probe_responses = *observer.probe_responses;
    let (_state, _platform, _observer) = phy.into_parts();
    let (control_tx, tx_summary) = scan_tx.into_parts();
    tx_storage
        .restore_control(control_tx)
        .unwrap_or_else(|_| panic!("cold scan returned over a live TX owner"));
    let tx_completions = tx_summary.completions;
    let tx_failures = tx_summary.failures;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=active-scan-timing channels={} elapsed_ms={}",
        STA_SCAN_CHANNEL_COUNT,
        scan_started.elapsed().as_millis(),
    ));

    let summary = scan_table.summary();
    let rx_dma_pass = summary.records != 0 && raw_frames != 0;
    let active_scan_pass =
        tx_completions >= STA_SCAN_CHANNEL_COUNT as u32 && probe_responses != 0 && tx_failures == 0;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-cold-scan-owner-return \
         records={} raw_frames={} ring_epochs={} probe_completions={} \
         probe_failures={} probe_responses={} rx_pass={} active_pass={}",
        summary.records,
        raw_frames,
        ring_epochs,
        tx_completions,
        tx_failures,
        probe_responses,
        u8::from(rx_dma_pass),
        u8::from(active_scan_pass),
    ));
    let target = primary_target
        .or_else(|| best_matching_ssid(scan_table.records(), network_credentials.ssid()).copied());
    let scan_ring = match scan_rx.into_halted() {
        Ok(ring) => ring,
        Err(rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=scan-rx-handoff phase={:?}",
                rx.phase(),
            ));
            return None;
        }
    };
    let descriptor_base = scan_ring.descriptor_base();
    let buffer_addresses = scan_ring.buffer_addresses();

    if rx_dma_pass && active_scan_pass {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=active-scan channels={} \
             tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} \
             records={} observed_frames={} raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    } else if rx_dma_pass {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels={} \
             dma=sram tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} records={} observed_frames={} \
             raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    } else {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-dma channels={} \
             tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} \
             records={} observed_frames={} raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    }

    Some(RadioHilColdScanHandoff {
        station_address,
        cold_mmio,
        rx: RadioHilJoinRx::from_halted(scan_ring),
        rx_storage: storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        ethernet_frame,
        target,
        scan_qualified: rx_dma_pass && active_scan_pass,
    })
}
