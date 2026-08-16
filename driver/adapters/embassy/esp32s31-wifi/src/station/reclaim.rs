//! Role-neutral PAC reclaim from a finite station lifecycle phase.
//!
//! Scan and initial-join phases own `RadioRuntimeOwner` directly. Connected
//! phases own the same PAC through one cooperative register arena. This module
//! normalizes those four clean frontiers without knowing HIL, board, network
//! or executor policy. A failed arena reclaim returns the exact original
//! phase; callers may retain it for board-level fault policy and cannot
//! mistake it for stopped Wi-Fi.

use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
use open_esp_radio_esp32s31_hal::radio_arena::{
    Esp32s31RadioOwnerArenaError, Esp32s31RadioOwnerRepublish,
};
use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio_esp32s31_wifi_mac::irq::MacInterruptRoute;
use open_esp_radio_esp32s31_wifi_sta::attempt::{
    Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation, Esp32s31StaIdentity,
};
use open_esp_radio_ieee80211::channel::{WifiChannel, WifiChannelError};
use open_esp_radio_wifi_embassy::station_network::{
    RunningStationNetwork, StationNetworkResources,
};

use crate::{
    embassy_irq::Esp32s31MacInterruptEpoch,
    rx_dma_service::{Esp32s31RxDmaStorage, Esp32s31RxEpochResources, Esp32s31StoppedRx},
    rx_frontier::{Esp32s31RxFrontier, Esp32s31RxFrontierDelay},
    scan_rx::Esp32s31ScanRx,
    station_epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch},
};

use super::{
    Esp32s31StationRoleOwner, Esp32s31StationRuntimeResources, Esp32s31StationServiceOwner,
    Esp32s31StationServicePhase, Esp32s31StationStorageResources,
};

/// Exact non-PAC resources retained at the phase where station stop landed.
///
/// Connected variants carry the arena republish capability beside all
/// protocol owners. Restarting STA can republish the returned PAC into that
/// exact arena; another role may instead retain these resources while it owns
/// the role-neutral Wi-Fi frontier.
pub enum Esp32s31StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E> {
    InitialScan {
        receive: S,
        network: N,
        identity: Esp32s31StaIdentity,
    },
    InitialJoin {
        receive: J,
        network: N,
        station: Esp32s31StaAttemptStation,
    },
    Disconnected {
        network: DN,
        receive: DR,
        aggregate_tx: A,
        control: C,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioOwnerRepublish<'arena>,
    },
    Reconnected {
        network: N,
        receive: J,
        rx: E,
        aggregate_tx: A,
        control: C,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioOwnerRepublish<'arena>,
    },
}

/// PAC and role-local resources returned by one successful phase reclaim.
pub struct Esp32s31StationPhaseReclaimed<R> {
    registers: RadioRuntimeOwner,
    resources: R,
    primary_channel: Option<WifiChannel>,
}

impl<R> Esp32s31StationPhaseReclaimed<R> {
    pub fn into_parts(self) -> (RadioRuntimeOwner, R, Option<WifiChannel>) {
        (self.registers, self.resources, self.primary_channel)
    }
}

/// Why a finite station phase could not be normalized into a reusable PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationPhaseReclaimError {
    Registers(Esp32s31RadioOwnerArenaError),
    InvalidChannel(WifiChannelError),
    /// The phase still owns an active connected data-plane transaction and
    /// therefore has not reached a reclaimable stop frontier.
    ConnectedActive,
}

/// Failed phase reclaim with the exact original phase still retained.
pub struct Esp32s31StationPhaseReclaimFailure<P> {
    pub error: Esp32s31StationPhaseReclaimError,
    pub phase: P,
}

/// Failed republish while rebuilding a previously stopped station phase.
///
/// Both the role-neutral PAC and every role-local resource are returned. A
/// caller may retain this quarantined frontier but cannot lose the arena token
/// or retry with a newly fabricated register owner.
pub struct Esp32s31StationPhaseRestoreFailure<R> {
    pub error: Esp32s31RadioOwnerArenaError,
    pub registers: RadioRuntimeOwner,
    pub resources: R,
}

/// A supposedly stopped phase could not be normalized for a new station
/// request. The complete original resource graph is retained.
pub struct Esp32s31StationPhaseRebindFailure<R> {
    pub resources: R,
}

type Esp32s31RebindableStationPhase<
    'arena,
    'storage,
    'pool,
    'queue,
    PD,
    RD,
    M,
    ND,
    NR,
    NS,
    A,
    C,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> = Esp32s31StationStoppedPhaseResources<
    'arena,
    Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    Esp32s31RxFrontier<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
    StationNetworkResources<ND, NR, NS>,
    RunningStationNetwork<NS, NR>,
    Esp32s31StoppedRx<
        'storage,
        'pool,
        'queue,
        RD,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    A,
    C,
    Esp32s31RxEpochResources<
        'storage,
        'pool,
        'queue,
        RD,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
>;

/// Minimum interrupt-epoch evidence needed before PAC reclaim may begin.
///
/// This is deliberately an observation, not an API for disabling an IRQ.
/// The active role must quiesce its route before returning its lifecycle
/// owner; reclaim refuses to repair or weaken that contract afterward.
pub trait Esp32s31StationInterruptEpochState {
    fn is_active(&self) -> bool;
}

impl<R, M> Esp32s31StationInterruptEpochState for Esp32s31MacInterruptEpoch<'_, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
{
    fn is_active(&self) -> bool {
        Esp32s31MacInterruptEpoch::is_active(self)
    }
}

/// Complete role and reusable station graph after phase and PAC reclaim.
///
/// The concrete composition still owns the final interrupt-route extraction:
/// this value merely proves the epoch reported inactive before it moved. It
/// retains every exact resource required either to reassemble `WifiStopped`
/// or to restart the returned station phase later.
pub struct Esp32s31StationRuntimeReclaimed<
    'storage,
    'security,
    P,
    I,
    D,
    T,
    B,
    S,
    const RECORDS: usize,
> {
    registers: RadioRuntimeOwner,
    role: Esp32s31StationRoleOwner<P>,
    interrupt: I,
    storage: Esp32s31StationStorageResources<'storage, D, T, RECORDS>,
    board: B,
    phase: S,
    security: Esp32s31StaAttemptSecurity<'security>,
    primary_channel: Option<WifiChannel>,
}

impl<'storage, 'security, P, I, D, T, B, S, const RECORDS: usize>
    Esp32s31StationRuntimeReclaimed<'storage, 'security, P, I, D, T, B, S, RECORDS>
{
    #[allow(clippy::type_complexity)]
    pub fn into_parts(
        self,
    ) -> (
        RadioRuntimeOwner,
        Esp32s31StationRoleOwner<P>,
        I,
        Esp32s31StationStorageResources<'storage, D, T, RECORDS>,
        B,
        S,
        Esp32s31StaAttemptSecurity<'security>,
        Option<WifiChannel>,
    ) {
        (
            self.registers,
            self.role,
            self.interrupt,
            self.storage,
            self.board,
            self.phase,
            self.security,
            self.primary_channel,
        )
    }
}

/// Exact lifecycle owner retained when runtime reclaim cannot prove safety.
pub enum Esp32s31StationRuntimeReclaimFailure<O> {
    InterruptActive {
        owner: O,
    },
    Phase {
        error: Esp32s31StationPhaseReclaimError,
        owner: O,
    },
}

/// Consume any clean finite station phase and recover its sole PAC owner.
///
/// The returned channel is absent only when stop occurred during initial scan,
/// before a peer had been selected. The caller updates the role-neutral radio
/// context only after independently proving that its IRQ epoch is inactive.
#[allow(clippy::type_complexity)]
pub fn try_reclaim_esp32s31_station_phase<'arena, S, J, N, DN, DR, A, C, E, K>(
    phase: Esp32s31StationServicePhase<
        RadioRuntimeOwner,
        S,
        J,
        N,
        Esp32s31DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
        Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
        K,
    >,
) -> Result<
    Esp32s31StationPhaseReclaimed<
        Esp32s31StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
    >,
    Esp32s31StationPhaseReclaimFailure<
        Esp32s31StationServicePhase<
            RadioRuntimeOwner,
            S,
            J,
            N,
            Esp32s31DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
            Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
            K,
        >,
    >,
> {
    let primary_channel = match &phase {
        Esp32s31StationServicePhase::InitialScan { .. } => None,
        Esp32s31StationServicePhase::InitialJoin { station, .. }
        | Esp32s31StationServicePhase::RunningScan { station, .. }
        | Esp32s31StationServicePhase::Reconnected { station, .. } => {
            match WifiChannel::mhz20(station.access_point.channel) {
                Ok(channel) => Some(channel),
                Err(error) => {
                    return Err(Esp32s31StationPhaseReclaimFailure {
                        error: Esp32s31StationPhaseReclaimError::InvalidChannel(error),
                        phase,
                    });
                }
            }
        }
        Esp32s31StationServicePhase::Connected { .. } => {
            return Err(Esp32s31StationPhaseReclaimFailure {
                error: Esp32s31StationPhaseReclaimError::ConnectedActive,
                phase,
            });
        }
    };
    let (registers, resources, primary_channel) = match phase {
        Esp32s31StationServicePhase::InitialScan {
            hardware,
            receive,
            network,
            identity,
        } => (
            hardware,
            Esp32s31StationStoppedPhaseResources::InitialScan {
                receive,
                network,
                identity,
            },
            primary_channel,
        ),
        Esp32s31StationServicePhase::InitialJoin {
            hardware,
            receive,
            network,
            station,
        } => (
            hardware,
            Esp32s31StationStoppedPhaseResources::InitialJoin {
                receive,
                network,
                station,
            },
            primary_channel,
        ),
        Esp32s31StationServicePhase::RunningScan {
            disconnected,
            station,
        } => {
            let parts = disconnected.into_parts();
            let reclaimed = match parts.hardware.try_into_reclaimed_registers() {
                Ok(reclaimed) => reclaimed,
                Err((hardware, error)) => {
                    return Err(Esp32s31StationPhaseReclaimFailure {
                        error: Esp32s31StationPhaseReclaimError::Registers(error),
                        phase: Esp32s31StationServicePhase::RunningScan {
                            disconnected: Esp32s31DisconnectedStaEpoch::new(
                                parts.network,
                                hardware,
                                parts.rx,
                                parts.aggregate_tx,
                                parts.control,
                            ),
                            station,
                        },
                    });
                }
            };
            let (registers, republish) = reclaimed.into_parts();
            (
                registers,
                Esp32s31StationStoppedPhaseResources::Disconnected {
                    network: parts.network,
                    receive: parts.rx,
                    aggregate_tx: parts.aggregate_tx,
                    control: parts.control,
                    station,
                    registers: republish,
                },
                primary_channel,
            )
        }
        Esp32s31StationServicePhase::Reconnected {
            epoch,
            network,
            station,
        } => {
            let parts = epoch.into_parts();
            let reclaimed = match parts.hardware.try_into_reclaimed_registers() {
                Ok(reclaimed) => reclaimed,
                Err((hardware, error)) => {
                    return Err(Esp32s31StationPhaseReclaimFailure {
                        error: Esp32s31StationPhaseReclaimError::Registers(error),
                        phase: Esp32s31StationServicePhase::Reconnected {
                            epoch: Esp32s31ReconnectedStaEpoch::new(
                                hardware,
                                parts.rx,
                                parts.rx_resources,
                                parts.aggregate_tx,
                                parts.control,
                            ),
                            network,
                            station,
                        },
                    });
                }
            };
            let (registers, republish) = reclaimed.into_parts();
            (
                registers,
                Esp32s31StationStoppedPhaseResources::Reconnected {
                    network,
                    receive: parts.rx,
                    rx: parts.rx_resources,
                    aggregate_tx: parts.aggregate_tx,
                    control: parts.control,
                    station,
                    registers: republish,
                },
                primary_channel,
            )
        }
        Esp32s31StationServicePhase::Connected { connected } => {
            return Err(Esp32s31StationPhaseReclaimFailure {
                error: Esp32s31StationPhaseReclaimError::ConnectedActive,
                phase: Esp32s31StationServicePhase::Connected { connected },
            });
        }
    };
    Ok(Esp32s31StationPhaseReclaimed {
        registers,
        resources,
        primary_channel,
    })
}

/// Republish a role-neutral PAC into the exact arena retained by a stopped
/// connected phase, or directly restore a scan/join phase.
#[allow(clippy::type_complexity)]
pub fn try_restore_esp32s31_station_phase<'arena, S, J, N, DN, DR, A, C, E, K>(
    registers: RadioRuntimeOwner,
    resources: Esp32s31StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
) -> Result<
    Esp32s31StationServicePhase<
        RadioRuntimeOwner,
        S,
        J,
        N,
        Esp32s31DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
        Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
        K,
    >,
    Esp32s31StationPhaseRestoreFailure<
        Esp32s31StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
    >,
> {
    match resources {
        Esp32s31StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        } => Ok(Esp32s31StationServicePhase::InitialScan {
            hardware: registers,
            receive,
            network,
            identity,
        }),
        Esp32s31StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => Ok(Esp32s31StationServicePhase::InitialJoin {
            hardware: registers,
            receive,
            network,
            station,
        }),
        Esp32s31StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            station,
            registers: republish,
        } => match republish.try_publish(registers) {
            Ok(published) => Ok(Esp32s31StationServicePhase::RunningScan {
                disconnected: Esp32s31DisconnectedStaEpoch::new(
                    network,
                    CooperativeRadioHardware::new(published),
                    receive,
                    aggregate_tx,
                    control,
                ),
                station,
            }),
            Err(failure) => Err(Esp32s31StationPhaseRestoreFailure {
                error: failure.error,
                registers: failure.owner,
                resources: Esp32s31StationStoppedPhaseResources::Disconnected {
                    network,
                    receive,
                    aggregate_tx,
                    control,
                    station,
                    registers: failure.republish,
                },
            }),
        },
        Esp32s31StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            station,
            registers: republish,
        } => match republish.try_publish(registers) {
            Ok(published) => Ok(Esp32s31StationServicePhase::Reconnected {
                epoch: Esp32s31ReconnectedStaEpoch::new(
                    CooperativeRadioHardware::new(published),
                    receive,
                    rx,
                    aggregate_tx,
                    control,
                ),
                network,
                station,
            }),
            Err(failure) => Err(Esp32s31StationPhaseRestoreFailure {
                error: failure.error,
                registers: failure.owner,
                resources: Esp32s31StationStoppedPhaseResources::Reconnected {
                    network,
                    receive,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers: failure.republish,
                },
            }),
        },
    }
}

/// Normalize a stopped station phase for a fresh SSID/security request.
///
/// Initial join discards its old candidate and returns to initial scan.
/// Disconnected and reconnected phases retain their already-created network,
/// aggregate and RX infrastructure but enter running scan, where the next
/// candidate replaces the old peer before join. No DMA operation is started
/// or stopped here.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn try_rebind_esp32s31_station_phase<
    'arena,
    'storage,
    'pool,
    'queue,
    PD,
    RD,
    M,
    ND,
    NR,
    NS,
    A,
    C,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    resources: Esp32s31RebindableStationPhase<
        'arena,
        'storage,
        'pool,
        'queue,
        PD,
        RD,
        M,
        ND,
        NR,
        NS,
        A,
        C,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    identity: Esp32s31StaIdentity,
) -> Result<
    Esp32s31RebindableStationPhase<
        'arena,
        'storage,
        'pool,
        'queue,
        PD,
        RD,
        M,
        ND,
        NR,
        NS,
        A,
        C,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    Esp32s31StationPhaseRebindFailure<
        Esp32s31RebindableStationPhase<
            'arena,
            'storage,
            'pool,
            'queue,
            PD,
            RD,
            M,
            ND,
            NR,
            NS,
            A,
            C,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    >,
>
where
    PD: Esp32s31RxFrontierDelay,
    M: RawMutex,
{
    match resources {
        Esp32s31StationStoppedPhaseResources::InitialScan {
            receive, network, ..
        } => Ok(Esp32s31StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        }),
        Esp32s31StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => match receive.try_into_halted() {
            Ok(ring) => Ok(Esp32s31StationStoppedPhaseResources::InitialScan {
                receive: Esp32s31ScanRx::from_halted(ring, storage),
                network,
                identity,
            }),
            Err(receive) => Err(Esp32s31StationPhaseRebindFailure {
                resources: Esp32s31StationStoppedPhaseResources::InitialJoin {
                    receive,
                    network,
                    station,
                },
            }),
        },
        Esp32s31StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            mut station,
            registers,
        } => {
            station.station_address = identity.station_address;
            station.association_preference = identity.association_preference;
            Ok(Esp32s31StationStoppedPhaseResources::Disconnected {
                network,
                receive,
                aggregate_tx,
                control,
                station,
                registers,
            })
        }
        Esp32s31StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            mut station,
            registers,
        } => {
            let network = match network {
                StationNetworkResources::Running(network) => network,
                network @ StationNetworkResources::Unstarted { .. } => {
                    return Err(Esp32s31StationPhaseRebindFailure {
                        resources: Esp32s31StationStoppedPhaseResources::Reconnected {
                            network,
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                    });
                }
            };
            let ring = match receive.try_into_halted() {
                Ok(ring) => ring,
                Err(receive) => {
                    return Err(Esp32s31StationPhaseRebindFailure {
                        resources: Esp32s31StationStoppedPhaseResources::Reconnected {
                            network: StationNetworkResources::Running(network),
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                    });
                }
            };
            station.station_address = identity.station_address;
            station.association_preference = identity.association_preference;
            Ok(Esp32s31StationStoppedPhaseResources::Disconnected {
                network,
                receive: rx.with_halted_ring(ring),
                aggregate_tx,
                control,
                station,
                registers,
            })
        }
    }
}

/// Reclaim one complete station lifecycle owner without losing a faulted
/// frontier.
///
/// This is the reusable boundary required by both production firmware and
/// HIL. It first verifies the returned IRQ epoch, then normalizes the phase's
/// direct or arena-backed PAC owner, and only afterward decomposes persistent
/// runtime resources. Both failure variants retain the original owner.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn try_reclaim_esp32s31_station_runtime<
    'role,
    'storage,
    'security,
    'arena,
    P,
    I,
    D,
    T,
    B,
    S,
    J,
    N,
    DN,
    DR,
    A,
    C,
    E,
    K,
    const RECORDS: usize,
>(
    owner: Esp32s31StationServiceOwner<
        'security,
        Esp32s31StationRuntimeResources<
            'role,
            'storage,
            Esp32s31StationRoleOwner<P>,
            I,
            D,
            T,
            B,
            RECORDS,
        >,
        Esp32s31StationServicePhase<
            RadioRuntimeOwner,
            S,
            J,
            N,
            Esp32s31DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
            Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
            K,
        >,
    >,
) -> Result<
    Esp32s31StationRuntimeReclaimed<
        'storage,
        'security,
        P,
        I,
        D,
        T,
        B,
        Esp32s31StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
        RECORDS,
    >,
    Esp32s31StationRuntimeReclaimFailure<
        Esp32s31StationServiceOwner<
            'security,
            Esp32s31StationRuntimeResources<
                'role,
                'storage,
                Esp32s31StationRoleOwner<P>,
                I,
                D,
                T,
                B,
                RECORDS,
            >,
            Esp32s31StationServicePhase<
                RadioRuntimeOwner,
                S,
                J,
                N,
                Esp32s31DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
                Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
                K,
            >,
        >,
    >,
>
where
    I: Esp32s31StationInterruptEpochState,
{
    if owner.runtime.radio().interrupt().is_active() {
        return Err(Esp32s31StationRuntimeReclaimFailure::InterruptActive { owner });
    }
    let (runtime, phase, security) = owner.into_parts();
    let reclaimed = match try_reclaim_esp32s31_station_phase(phase) {
        Ok(reclaimed) => reclaimed,
        Err(failure) => {
            return Err(Esp32s31StationRuntimeReclaimFailure::Phase {
                error: failure.error,
                owner: Esp32s31StationServiceOwner::new(runtime, failure.phase, security),
            });
        }
    };
    let (registers, phase, primary_channel) = reclaimed.into_parts();
    let runtime = runtime.into_parts();
    let (role, interrupt) = runtime.radio.into_parts();
    Ok(Esp32s31StationRuntimeReclaimed {
        registers,
        role,
        interrupt,
        storage: runtime.storage,
        board: runtime.board,
        phase,
        security,
        primary_channel,
    })
}
