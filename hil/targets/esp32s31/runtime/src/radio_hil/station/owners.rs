#![forbid(unsafe_code)]

use embassy_executor::{SendSpawner, Spawner};
use embassy_net::Stack;
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        aggregate_tx::{AggregateTxError, AggregateTxResetReason},
        connected_services::Esp32s31ConnectedServicesError,
        single_mpdu_tx::{SingleMpduTxError, TxResetReason},
        station::Esp32s31StationCommand,
    },
    esp32s31::{
        hal::RadioRegisters,
        phy::phy_cold::PhyColdState,
        wifi::lmac::{
            irq::{MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT},
            scan::{ScanRecord, ScanTable},
        },
    },
    wifi::{ieee80211::station::StaTxSequenceCounters, wpa2::Pmk},
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;

use super::super::{
    NetworkDevice, NetworkRunner, RX_DESCRIPTOR_COUNT, RadioHilDisconnectedEpoch, RadioHilJoinRx,
    RadioHilMacInterruptEpoch, RadioHilReconnectedEpoch, RxStorage, TxStorage,
};
use crate::radio_fault::ArmedStationFault;
use open_esp_radio::esp32s31::wifi::sta::peer::Esp32s31ConnectedStaPeer;

use super::{
    connected_epoch::{RadioHilConnectedEpochBindings, RadioHilConnectedTaskBindings},
    connected_rx_observer::RadioHilConnectedRxBindings,
    network_reporting::RadioHilNetworkReportBindings,
};

/// Hardware/storage input for one production connected epoch.
///
/// Only the first variant may initialize static cells. The reconnect variant
/// is assembled exclusively from a completed disconnected epoch, making a
/// second `StaticCell::init` structurally impossible.
pub(in crate::radio_hil) enum RadioHilConnectedEpochResources {
    Initial {
        registers: &'static mut RadioRegisters,
        rx: RadioHilJoinRx<'static>,
    },
    Reconnected(RadioHilReconnectedEpoch),
}

/// Board and station state returned after all connected tasks have stopped.
pub(in crate::radio_hil) struct RadioHilConnectedEpochReturn<'fixture, 'security> {
    pub fixture: RadioHilConnectedTaskFixture<'fixture>,
    pub disconnected: RadioHilDisconnectedEpoch,
    pub security: StaAssociationSecurity<'security>,
    pub exit: RadioHilConnectedExit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilConnectedExit {
    Disconnected {
        beacon_lost: bool,
    },
    ReconnectRequested,
    StationStopped(Esp32s31StationCommand),
    InjectedTxFault {
        fault: ArmedStationFault,
        reset_required: bool,
    },
    HardwareFailure,
}

pub(in crate::radio_hil) fn injected_tx_source_requires_reset<R, C>(
    source: &Esp32s31ConnectedServicesError<R, C, AggregateTxError>,
) -> bool {
    let expected_events = MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT;
    matches!(
        source,
        Esp32s31ConnectedServicesError::Tx(AggregateTxError::RadioResetRequired(
            AggregateTxResetReason::ConflictingInterruptEvents(events),
        )) if *events == expected_events
    ) || matches!(
        source,
        Esp32s31ConnectedServicesError::Tx(AggregateTxError::Ordinary(
            SingleMpduTxError::RadioResetRequired(TxResetReason::ConflictingInterruptEvents(
                events,
            )),
        )) if *events == expected_events
    )
}

pub(in crate::radio_hil) struct RadioHilReconnectReady<'fixture, 'security> {
    pub fixture: RadioHilConnectedTaskFixture<'fixture>,
    pub target: StaJoinTarget,
    pub network: RadioHilStaNetwork,
    pub epoch: RadioHilConnectedEpochResources,
    pub security: StaAssociationSecurity<'security>,
}

pub(in crate::radio_hil) struct RadioHilRunningScanReady<'fixture, 'security> {
    pub fixture: RadioHilConnectedTaskFixture<'fixture>,
    pub previous_target: StaJoinTarget,
    pub disconnected: RadioHilDisconnectedEpoch,
    pub security: StaAssociationSecurity<'security>,
}

pub(in crate::radio_hil) struct StaConnectedSession<'security> {
    pub generation: u32,
    pub peer: Esp32s31ConnectedStaPeer,
    pub network: RadioHilStaNetwork,
    pub pmk: &'security Pmk,
    pub supplicant_nonce: [u8; 32],
    pub sequences: &'security mut StaTxSequenceCounters,
}

pub(in crate::radio_hil) enum RadioHilStaNetwork {
    Unstarted {
        device: NetworkDevice,
        runner: NetworkRunner,
    },
    Running(RadioHilRunningNetwork),
}

pub(in crate::radio_hil) struct RadioHilRunningNetwork {
    pub stack: Stack<'static>,
    pub runner: NetworkRunner,
}

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct StaJoinTarget {
    pub station_address: [u8; 6],
    pub access_point: ScanRecord,
}

pub(in crate::radio_hil) struct StaAssociationSecurity<'a> {
    pub pmk: &'a Pmk,
    pub supplicant_nonce: [u8; 32],
    pub sequences: &'a mut StaTxSequenceCounters,
}

pub(in crate::radio_hil) enum RadioHilStaLifecycleOwner<'fixture, 'security> {
    Authenticate(RadioHilAuthenticationReady<'fixture, 'security>),
    RunningScan(RadioHilRunningScanReady<'fixture, 'security>),
    Reconnect(RadioHilReconnectReady<'fixture, 'security>),
}

pub(in crate::radio_hil) struct RadioHilConnectedFixture<'a> {
    pub spawner: Spawner,
    pub protocol_spawner: SendSpawner,
    pub state: &'a mut PhyColdState,
    pub platform: &'a mut EspHalRadioPeripheral,
    pub mmio: &'static mut RadioRegisters,
    pub interrupt_epoch: &'a mut RadioHilMacInterruptEpoch,
    pub rx_storage: &'static RxStorage,
    pub tx_storage: &'static mut TxStorage,
    pub descriptor_base: u32,
    pub buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    pub scan_table: &'static mut ScanTable,
    pub frame: &'static mut [u8],
    pub ethernet: &'static mut [u8],
    pub connected_tasks: RadioHilConnectedTaskBindings,
    pub connected_rx: RadioHilConnectedRxBindings,
    pub network_report: RadioHilNetworkReportBindings,
    pub connected_epoch: RadioHilConnectedEpochBindings,
}

pub(in crate::radio_hil) struct RadioHilConnectedTaskFixture<'a> {
    pub spawner: Spawner,
    pub protocol_spawner: SendSpawner,
    pub state: &'a mut PhyColdState,
    pub platform: &'a mut EspHalRadioPeripheral,
    pub interrupt_epoch: &'a mut RadioHilMacInterruptEpoch,
    pub rx_storage: &'static RxStorage,
    pub tx_storage: &'static mut TxStorage,
    pub descriptor_base: u32,
    pub buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    pub scan_table: &'static mut ScanTable,
    pub frame: &'static mut [u8],
    pub ethernet: &'static mut [u8],
    pub connected_tasks: RadioHilConnectedTaskBindings,
    pub connected_rx: RadioHilConnectedRxBindings,
    pub network_report: RadioHilNetworkReportBindings,
    pub connected_epoch: RadioHilConnectedEpochBindings,
}

impl<'a> RadioHilConnectedFixture<'a> {
    pub(in crate::radio_hil) fn into_task_fixture(
        self,
    ) -> (
        RadioHilConnectedTaskFixture<'a>,
        &'static mut RadioRegisters,
    ) {
        (
            RadioHilConnectedTaskFixture {
                spawner: self.spawner,
                protocol_spawner: self.protocol_spawner,
                state: self.state,
                platform: self.platform,
                interrupt_epoch: self.interrupt_epoch,
                rx_storage: self.rx_storage,
                tx_storage: self.tx_storage,
                descriptor_base: self.descriptor_base,
                buffer_addresses: self.buffer_addresses,
                scan_table: self.scan_table,
                frame: self.frame,
                ethernet: self.ethernet,
                connected_tasks: self.connected_tasks,
                connected_rx: self.connected_rx,
                network_report: self.network_report,
                connected_epoch: self.connected_epoch,
            },
            self.mmio,
        )
    }
}

pub(in crate::radio_hil) struct RadioHilAuthenticationReady<'fixture, 'security> {
    pub fixture: RadioHilConnectedFixture<'fixture>,
    pub target: StaJoinTarget,
    pub rx: RadioHilJoinRx<'static>,
    pub network: RadioHilStaNetwork,
    pub security: StaAssociationSecurity<'security>,
}
