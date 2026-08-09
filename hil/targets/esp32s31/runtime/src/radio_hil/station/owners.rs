#![forbid(unsafe_code)]

use embassy_executor::{SendSpawner, Spawner};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::esp32s31::{
    hal::RadioRegisters,
    wifi::device::register_arena::Esp32s31RadioRegistersArenaError,
    wifi::mac::irq::{MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT},
    wifi::sta::attempt::Esp32s31StaAttemptSecurity,
    wifi::sta::single_mpdu_tx::{SingleMpduTxError, TxResetReason},
};
use open_esp_radio_esp32s31_wifi_embassy::{
    aggregate_tx::{AggregateTxError, AggregateTxResetReason},
    connected_services::Esp32s31ConnectedServicesError,
    connected_sta_port::Esp32s31ConnectedStaConfig,
    station::{
        Esp32s31ConnectedEpochResources, Esp32s31ConnectedServiceResources, Esp32s31StationCommand,
        Esp32s31StationControlResources, Esp32s31StationDmaResources,
        Esp32s31StationPhaseReclaimError, Esp32s31StationRadioResources, Esp32s31StationRoleOwner,
        Esp32s31StationRuntimeReclaimFailure, Esp32s31StationRuntimeResources,
        Esp32s31StationServiceOwner, Esp32s31StationServicePhase,
        Esp32s31StationStoppedPhaseResources, Esp32s31StationStorageResources,
        try_reclaim_esp32s31_station_runtime, try_restore_esp32s31_station_phase,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_wifi_embassy::station_network::{
    RunningStationNetwork, StationNetworkResources,
};

use super::super::{
    ConnectedAmpduStorage, ConnectedRxEpochResources, ConnectedStoppedRx, ControlResources,
    NetworkDevice, NetworkRunner, RX_DESCRIPTOR_COUNT, RadioHilDisconnectedEpoch, RadioHilJoinRx,
    RadioHilMacInterruptEpoch, RadioHilReconnectedEpoch, RxStorage, ScanRx, TxStorage,
};
use crate::radio_fault::ArmedStationFault;
use open_esp_radio::wifi::ieee80211::scan::SCAN_RECORD_CAPACITY;
use open_esp_radio::wifi::softmac::interface::BoundVirtualInterface;

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
pub(in crate::radio_hil) type RadioHilConnectedEpochResources = Esp32s31ConnectedEpochResources<
    RadioRegisters,
    RadioHilJoinRx<'static>,
    RadioHilReconnectedEpoch,
>;

/// Board and station state returned after all connected tasks have stopped.
pub(in crate::radio_hil) struct RadioHilConnectedEpochReturn<'fixture, 'security> {
    pub fixture: RadioHilConnectedTaskFixture<'fixture>,
    pub disconnected: RadioHilDisconnectedEpoch,
    pub security: Esp32s31StaAttemptSecurity<'security>,
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

pub(in crate::radio_hil) type RadioHilConnectedServiceResources<'fixture, 'security> =
    Esp32s31ConnectedServiceResources<
        'security,
        RadioHilConnectedTaskFixture<'fixture>,
        RadioHilConnectedEpochResources,
        RadioHilStaNetwork,
    >;

pub(in crate::radio_hil) type RadioHilStaNetwork =
    StationNetworkResources<NetworkDevice, NetworkRunner, Stack<'static>>;

pub(in crate::radio_hil) type RadioHilRunningNetwork =
    RunningStationNetwork<Stack<'static>, NetworkRunner>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilStationReclaimError {
    InterruptActive,
    Phase(Esp32s31StationPhaseReclaimError),
}

pub(in crate::radio_hil) struct RadioHilStationReclaimed<'security> {
    pub registers: RadioRegisters,
    pub role: Esp32s31StationRoleOwner<EspHalRadioPeripheral>,
    pub interrupt: RadioHilMacInterruptEpoch,
    /// Selected peer channel when the terminal phase had a candidate.
    /// `InitialScan` deliberately has no fabricated peer/channel identity.
    pub channel: Option<open_esp_radio::wifi::ieee80211::channel::WifiChannel>,
    pub resources: RadioHilStationReusableResources<'security>,
}

pub(in crate::radio_hil) struct RadioHilStationRestartFailure<'security> {
    pub error: Esp32s31RadioRegistersArenaError,
    pub registers: RadioRegisters,
    pub resources: RadioHilStationReusableResources<'security>,
}

/// Station-owned executor/storage bindings which remain valid across role
/// epochs. Hardware, PHY state and the interrupt setup token are deliberately
/// absent; they reunite only in the role-neutral `WifiStopped` owner.
pub(in crate::radio_hil) struct RadioHilStationFixtureResources {
    storage: RadioHilStationStorageResources,
    board: RadioHilStationBoardResources,
}

pub(in crate::radio_hil) type RadioHilStationDmaResources =
    Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>;

pub(in crate::radio_hil) struct RadioHilStationBoardResources {
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    interface: BoundVirtualInterface,
    connected_tasks: RadioHilConnectedTaskBindings,
    connected_rx: RadioHilConnectedRxBindings,
    network_report: RadioHilNetworkReportBindings,
    connected_epoch: RadioHilConnectedEpochBindings,
    station_control: &'static Esp32s31StationControlResources<CriticalSectionRawMutex>,
}

impl RadioHilStationBoardResources {
    pub(in crate::radio_hil) const fn new(
        spawner: Spawner,
        protocol_spawner: SendSpawner,
        interface: BoundVirtualInterface,
        connected_tasks: RadioHilConnectedTaskBindings,
        connected_rx: RadioHilConnectedRxBindings,
        network_report: RadioHilNetworkReportBindings,
        connected_epoch: RadioHilConnectedEpochBindings,
        station_control: &'static Esp32s31StationControlResources<CriticalSectionRawMutex>,
    ) -> Self {
        Self {
            spawner,
            protocol_spawner,
            interface,
            connected_tasks,
            connected_rx,
            network_report,
            connected_epoch,
            station_control,
        }
    }

    pub(in crate::radio_hil) const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub(in crate::radio_hil) const fn connected_station_config(
        &self,
    ) -> Esp32s31ConnectedStaConfig {
        self.connected_epoch.policy.station
    }

    pub(in crate::radio_hil) const fn connected_epoch_bindings(
        &self,
    ) -> &RadioHilConnectedEpochBindings {
        &self.connected_epoch
    }

    pub(in crate::radio_hil) const fn station_control(
        &self,
    ) -> &'static Esp32s31StationControlResources<CriticalSectionRawMutex> {
        self.station_control
    }

    pub(in crate::radio_hil) fn into_parts(
        self,
    ) -> (
        Spawner,
        SendSpawner,
        BoundVirtualInterface,
        RadioHilConnectedTaskBindings,
        RadioHilConnectedRxBindings,
        RadioHilNetworkReportBindings,
        RadioHilConnectedEpochBindings,
        &'static Esp32s31StationControlResources<CriticalSectionRawMutex>,
    ) {
        (
            self.spawner,
            self.protocol_spawner,
            self.interface,
            self.connected_tasks,
            self.connected_rx,
            self.network_report,
            self.connected_epoch,
            self.station_control,
        )
    }
}

type RadioHilStationStorageResources = Esp32s31StationStorageResources<
    'static,
    RadioHilStationDmaResources,
    &'static mut TxStorage,
    SCAN_RECORD_CAPACITY,
>;

pub(in crate::radio_hil) type RadioHilConnectedTaskFixture<'a> = Esp32s31StationRuntimeResources<
    'a,
    'static,
    Esp32s31StationRoleOwner<EspHalRadioPeripheral>,
    RadioHilMacInterruptEpoch,
    RadioHilStationDmaResources,
    &'static mut TxStorage,
    RadioHilStationBoardResources,
    SCAN_RECORD_CAPACITY,
>;

pub(in crate::radio_hil) type RadioHilStationPhase = Esp32s31StationServicePhase<
    RadioRegisters,
    ScanRx,
    RadioHilJoinRx<'static>,
    RadioHilStaNetwork,
    RadioHilDisconnectedEpoch,
    RadioHilReconnectedEpoch,
>;

pub(in crate::radio_hil) type RadioHilStaLifecycleOwner<'fixture, 'security> =
    Esp32s31StationServiceOwner<
        'security,
        RadioHilConnectedTaskFixture<'fixture>,
        RadioHilStationPhase,
    >;

/// Exact non-hardware frontier returned by the phase in which station stop
/// was observed. No variant fabricates a fresh DMA, network or control owner.
pub(in crate::radio_hil) type RadioHilStationStoppedPhaseResources =
    Esp32s31StationStoppedPhaseResources<
        'static,
        ScanRx,
        RadioHilJoinRx<'static>,
        RadioHilStaNetwork,
        RadioHilRunningNetwork,
        ConnectedStoppedRx,
        ConnectedAmpduStorage,
        &'static ControlResources,
        ConnectedRxEpochResources,
    >;

/// Complete role-local resource graph returned by a clean station stop.
///
/// This remains opaque until the next materialization path consumes each
/// variant. Merely possessing static allocation addresses would not prove
/// that the live phase owners had returned; this value contains those owners.
pub(in crate::radio_hil) struct RadioHilStationReusableResources<'security> {
    fixture: RadioHilStationFixtureResources,
    phase: RadioHilStationStoppedPhaseResources,
    security: Esp32s31StaAttemptSecurity<'security>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilStationStoppedPhase {
    Scanning,
    Initial,
    Disconnected,
    Reconnected,
}

impl RadioHilStationReusableResources<'_> {
    /// Classify the returned frontier while borrowing every retained owner.
    ///
    /// The borrows are intentional: this evidence is emitted before the
    /// resource graph moves into another role materialization or board reset.
    pub(in crate::radio_hil) fn stopped_phase(&self) -> RadioHilStationStoppedPhase {
        let _retained_storage = self.fixture.storage.parts();
        let RadioHilStationBoardResources {
            spawner,
            protocol_spawner,
            interface,
            connected_tasks,
            connected_rx,
            network_report,
            connected_epoch,
            station_control,
        } = &self.fixture.board;
        let _retained_board = (
            spawner,
            protocol_spawner,
            interface,
            connected_tasks,
            connected_rx,
            network_report,
            connected_epoch,
            station_control,
        );
        let _retained_security = &self.security;
        match &self.phase {
            RadioHilStationStoppedPhaseResources::InitialScan {
                receive,
                network,
                identity,
            } => {
                let _returned = (receive, network, identity);
                RadioHilStationStoppedPhase::Scanning
            }
            RadioHilStationStoppedPhaseResources::InitialJoin {
                receive,
                network,
                station,
            } => {
                let _returned = (receive, network, station);
                RadioHilStationStoppedPhase::Initial
            }
            RadioHilStationStoppedPhaseResources::Disconnected {
                network,
                receive,
                aggregate_tx,
                control,
                station,
                registers,
            } => {
                let _returned = (network, receive, aggregate_tx, control, station, registers);
                RadioHilStationStoppedPhase::Disconnected
            }
            RadioHilStationStoppedPhaseResources::Reconnected {
                network,
                receive,
                rx,
                aggregate_tx,
                control,
                station,
                registers,
            } => {
                let _returned = (
                    network,
                    receive,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers,
                );
                RadioHilStationStoppedPhase::Reconnected
            }
        }
    }
}

/// Rebind a clean role-neutral owner to the exact STA phase/resources returned
/// by a preceding task. No static cell, PAC singleton or protocol identity is
/// reconstructed at this edge.
pub(in crate::radio_hil) fn try_restart_station_runtime<'fixture, 'security>(
    role: Esp32s31StationRoleOwner<EspHalRadioPeripheral>,
    interrupt_epoch: RadioHilMacInterruptEpoch,
    registers: RadioRegisters,
    resources: RadioHilStationReusableResources<'security>,
) -> Result<
    (
        RadioHilStaLifecycleOwner<'fixture, 'security>,
        &'static Esp32s31StationControlResources<CriticalSectionRawMutex>,
    ),
    RadioHilStationRestartFailure<'security>,
> {
    let RadioHilStationReusableResources {
        fixture,
        phase,
        security,
    } = resources;
    let phase = match try_restore_esp32s31_station_phase(registers, phase) {
        Ok(phase) => phase,
        Err(failure) => {
            return Err(RadioHilStationRestartFailure {
                error: failure.error,
                registers: failure.registers,
                resources: RadioHilStationReusableResources {
                    fixture,
                    phase: failure.resources,
                    security,
                },
            });
        }
    };
    let RadioHilStationFixtureResources { storage, board } = fixture;
    let station_control = board.station_control();
    let runtime = Esp32s31StationRuntimeResources::new(
        Esp32s31StationRadioResources::new(role, interrupt_epoch),
        storage,
        board,
    );
    Ok((
        Esp32s31StationServiceOwner::new(runtime, phase, security),
        station_control,
    ))
}

/// Consume a clean finite STA frontier and recover the exact PAC owner.
///
/// This is intentionally unavailable as a fallback for cancellation or a
/// live interrupt epoch. A published register lease which cannot be
/// reclaimed is dropped fail-closed and poisons its arena for reset.
pub(in crate::radio_hil) fn try_reclaim_station_runtime<'security>(
    owner: RadioHilStaLifecycleOwner<'_, 'security>,
) -> Result<RadioHilStationReclaimed<'security>, RadioHilStationReclaimError> {
    let reclaimed =
        try_reclaim_esp32s31_station_runtime(owner).map_err(|failure| match failure {
            Esp32s31StationRuntimeReclaimFailure::InterruptActive { .. } => {
                RadioHilStationReclaimError::InterruptActive
            }
            Esp32s31StationRuntimeReclaimFailure::Phase { error, .. } => {
                RadioHilStationReclaimError::Phase(error)
            }
        })?;
    let (registers, role, interrupt, storage, board, stopped_phase, security, channel) =
        reclaimed.into_parts();
    Ok(RadioHilStationReclaimed {
        registers,
        role,
        interrupt,
        channel,
        resources: RadioHilStationReusableResources {
            fixture: RadioHilStationFixtureResources { storage, board },
            phase: stopped_phase,
            security,
        },
    })
}
