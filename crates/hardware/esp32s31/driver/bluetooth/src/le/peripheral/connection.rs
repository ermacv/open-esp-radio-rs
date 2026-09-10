//! Production ownership for the first ESP32-S31 BLE peripheral connection.
//!
//! The runtime joins a portable LL event to the recovered allocation graph,
//! installs its reviewed Access Address and CRCInit fields and attaches one
//! separately owned static non-scanning RX pool. It cannot publish that partial
//! graph; direction-finding workspace policy and scheduler admission must be
//! closed first.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
mod completion;
#[cfg(any(target_arch = "riscv32", test))]
mod recurring;

#[cfg(target_arch = "riscv32")]
use oer_bluetooth_ll::connection::LePeripheralConnectionEventInFlight;

use oer_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeConnectionTiming, LeDataChannelIndex,
    LePeripheralConnection, LePeripheralConnectionEventPrepared,
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{
    DirectionFindingWorkspaceLink, LeReceivedPdu, PeripheralConnectionDataChannel,
    PeripheralConnectionEventSpan, PeripheralConnectionMemoryGraphDirectionFindingPrepared,
    PeripheralConnectionMemoryGraphEventFieldsPrepared,
    PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared, PeripheralConnectionReceiveTime,
    PeripheralConnectionReceiveWait, PeripheralConnectionSchedulerPriority,
    PeripheralConnectionSchedulerWindow,
};

use oer_esp32s31_bluetooth_memory::{
    NonScanningRxMemoryBindFailure, NonScanningRxMemoryCpuOwned, NonScanningRxMemoryIdentity,
    NonScanningRxMemoryStorage, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionIdentity, PeripheralConnectionMemoryGraphBindFailure,
    PeripheralConnectionMemoryGraphCpuOwned, PeripheralConnectionMemoryGraphIdentity,
    PeripheralConnectionMemoryGraphIdentityPrepared,
    PeripheralConnectionMemoryGraphReceivePrepared, PeripheralConnectionMemoryGraphStorage,
};
#[cfg(not(target_arch = "riscv32"))]
use oer_esp32s31_bluetooth_memory::{
    NonScanningRxMemoryModelAddress, PeripheralConnectionMemoryGraphModelAddress,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    PeripheralConnectionMemoryGraphCompletionObservation,
    PeripheralConnectionMemoryGraphPublicationPrepared, PeripheralConnectionMemoryGraphRunning,
    PeripheralConnectionMemoryGraphRxPublished,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerFinishedHardwareListObserved;

use crate::SchedulerInstant;
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    ControllerSchedulerEpoch,
    scheduler::{SchedulerRawWindow, SchedulerSoftwareConfig},
};
#[cfg(target_arch = "riscv32")]
pub(crate) use completion::{
    PeripheralConnectionCompletedEvent, PeripheralConnectionCompletedEventRecurringParts,
    PeripheralConnectionCompletedEventRecurringRemainder,
    PeripheralConnectionCompletionClassification,
    PeripheralConnectionFirstEventCompletionObservation,
    PeripheralConnectionFirstEventCompletionObserved, PeripheralConnectionRecycledEvent,
};
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use recurring::PeripheralConnectionRecurringPhase;
#[cfg(any(target_arch = "riscv32", test))]
pub use recurring::PeripheralConnectionRecurringTimingError;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use recurring::{
    PeripheralConnectionLocalSleepClockAccuracy, PeripheralConnectionRecurringTimingPolicy,
    PeripheralConnectionWindowWideningMode,
};

// Source-owned S31 first-event policy. The 5,154-us LE 1M reservation is
// retained by both current and named older S31 controller bodies. The 16-us
// uncertainty is the open NimBLE timing guard. They are backend scheduling
// policy, not portable Link Layer fields and not a vendor aggregate ABI.
const LE_1M_FIRST_EVENT_RESERVATION_MICROS: u32 = 5_154;
// The reviewed S31 connection LLL reserves this fixed common preparation
// duration before projecting the remaining connection interval into the
// hardware-owned event-span input.
#[cfg(any(target_arch = "riscv32", test))]
const LE_CONNECTION_COMMON_RESERVE_MICROS: u32 = 440;
#[cfg(any(target_arch = "riscv32", test))]
const LE_FIRST_EVENT_TIMING_GUARD_MICROS: u32 = 16;
const LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS: u32 = 1;

/// PHY-calibrated on-air start of one received LE 1M packet.
///
/// Only the initialized S31 BLE PHY timing authority can create this value
/// from a hardware packet capture. It deliberately exposes no raw controller
/// ticks or scheduler image.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the packet-start timing must enter response admission or remain retained"]
pub struct Le1MPacketStartTiming {
    packet_start: SchedulerInstant,
}

impl Le1MPacketStartTiming {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_scheduler_micros(micros: u32) -> Self {
        Self {
            packet_start: SchedulerInstant::from_image(micros),
        }
    }

    fn first_connection_window(
        self,
        timing: LeConnectionTiming,
    ) -> PeripheralConnectionFirstWindow {
        let packet_end = self
            .packet_start
            .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS);
        PeripheralConnectionFirstWindow {
            anchor: packet_end.wrapping_add(timing.first_window_start_micros()),
            receive_end: packet_end.wrapping_add(timing.first_window_end_micros()),
            event_end: packet_end
                .wrapping_add(timing.first_window_end_micros())
                .wrapping_add(LE_1M_FIRST_EVENT_RESERVATION_MICROS)
                .wrapping_add(LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS),
        }
    }
}

/// PHY-calibrated on-air packet start captured during one connection event.
///
/// This value is distinct from the advertising packet timing which forms the
/// first connection window. It remains bound to the recycled connection until
/// completion classification decides whether the captured phase may continue.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the connection packet-start timing must remain bound to its recycled event"]
pub struct PeripheralConnectionPacketStartTiming {
    packet_start: SchedulerInstant,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionPacketStartTiming {
    pub(crate) const fn from_scheduler_micros(micros: u32) -> Self {
        Self {
            packet_start: SchedulerInstant::from_image(micros),
        }
    }

    /// Borrow the normalized position only inside the chip timing boundary.
    pub(crate) const fn scheduler_instant(&self) -> SchedulerInstant {
        self.packet_start
    }

    #[cfg(test)]
    pub(crate) const fn elapsed_since(&self, earlier: &Self) -> u32 {
        self.packet_start
            .image()
            .wrapping_sub(earlier.packet_start.image())
    }
}

/// Absolute first receive window and containing event reservation.
///
/// The positions stay private to the S31 scheduler boundary. Portable Link
/// Layer code owns only the relative WinOffset/WinSize semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralConnectionFirstWindow {
    anchor: SchedulerInstant,
    receive_end: SchedulerInstant,
    event_end: SchedulerInstant,
}

impl PeripheralConnectionFirstWindow {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn anchor(self) -> SchedulerInstant {
        self.anchor
    }

    #[cfg(test)]
    pub(crate) const fn end(self) -> SchedulerInstant {
        self.receive_end
    }

    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        dead_code,
        reason = "the next connection scheduler-admission transition consumes this projection"
    )]
    const fn project_scheduler_window(
        self,
        epoch: ControllerSchedulerEpoch,
        config: SchedulerSoftwareConfig,
    ) -> Option<SchedulerRawWindow> {
        let scheduler_start = self
            .anchor
            .image()
            .wrapping_sub(config.preparation_lead_micros())
            .wrapping_sub(LE_FIRST_EVENT_TIMING_GUARD_MICROS)
            .wrapping_sub(LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS);
        SchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_ticks_for_micros(scheduler_start),
            epoch.raw_ticks_for_micros(self.event_end.image()),
        )
    }
}

/// Immutable physical policy retained with the connection allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionRuntimeConfig {
    default_tx_power_dbm: PeripheralConnectionDefaultTxPowerDbm,
    version_information: Option<oer_bluetooth_ll::control::LeVersionInformation>,
    #[cfg(any(target_arch = "riscv32", test))]
    recurring_timing_policy: Option<PeripheralConnectionRecurringTimingPolicy>,
}

impl PeripheralConnectionRuntimeConfig {
    /// Bind the physical default transmit-power request once per runtime.
    pub const fn new(default_tx_power_dbm: PeripheralConnectionDefaultTxPowerDbm) -> Self {
        Self {
            default_tx_power_dbm,
            version_information: None,
            #[cfg(any(target_arch = "riscv32", test))]
            recurring_timing_policy: None,
        }
    }

    /// Supply the identity of this Controller implementation for LL version exchange.
    pub const fn with_version_information(
        mut self,
        version: oer_bluetooth_ll::control::LeVersionInformation,
    ) -> Self {
        self.version_information = Some(version);
        self
    }

    pub const fn version_information(
        self,
    ) -> Option<oer_bluetooth_ll::control::LeVersionInformation> {
        self.version_information
    }

    /// Opt in to the reviewed software window-widening profile.
    ///
    /// The caller must own the physical worst-case accuracy of the selected
    /// local sleep clock. The ordinary constructor deliberately leaves
    /// recurrence unavailable rather than inferring accuracy from the clock
    /// source or selecting a vendor mode without authority.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn with_software_recurring_timing(
        mut self,
        local_sleep_clock_accuracy_ppm: u16,
    ) -> Option<Self> {
        let Some(local_sleep_clock_accuracy) =
            PeripheralConnectionLocalSleepClockAccuracy::new(local_sleep_clock_accuracy_ppm)
        else {
            return None;
        };
        self.recurring_timing_policy = Some(PeripheralConnectionRecurringTimingPolicy::new(
            Some(local_sleep_clock_accuracy),
            PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
        ));
        Some(self)
    }

    /// Physical default power used for every first-event descriptor.
    pub const fn default_tx_power_dbm(self) -> PeripheralConnectionDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn recurring_timing_policy(
        self,
    ) -> Option<PeripheralConnectionRecurringTimingPolicy> {
        self.recurring_timing_policy
    }
}

/// Exact CPU-owned graph and receive pool checked out from one runtime.
#[must_use = "the connection allocation must be restored or retained by an event typestate"]
pub struct PeripheralConnectionRuntimeAllocation {
    graph: PeripheralConnectionMemoryGraphCpuOwned,
    receive_pool: NonScanningRxMemoryCpuOwned,
}

/// Accepted `CONNECT_IND` and the exact peripheral allocation loaned to advertising.
///
/// The copied receive PDU is retained because its captured hardware time is the
/// causal input to the first connection window. This owner has not normalized
/// that time, published a connection scheduler item, or reported a connection.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "normalize the retained packet time and prepare the first connection event"]
pub(crate) struct PeripheralConnectionAcceptedRequest {
    allocation: PeripheralConnectionRuntimeAllocation,
    connection: LePeripheralConnection,
    packet: LeReceivedPdu,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionAcceptedRequest {
    pub(crate) fn new(
        allocation: PeripheralConnectionRuntimeAllocation,
        connection: LePeripheralConnection,
        packet: LeReceivedPdu,
    ) -> Self {
        Self {
            allocation,
            connection,
            packet,
        }
    }

    /// Exact copied packet whose captured time caused this connection request.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn packet(&self) -> &LeReceivedPdu {
        &self.packet
    }

    /// Join the normalized causal packet start to the existing first-event path.
    ///
    /// The copied packet remains a separate semantic owner so every
    /// pre-publication rejection can reconstruct this exact accepted request.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_first_event_parts(
        self,
        packet_start: Le1MPacketStartTiming,
    ) -> (PeripheralConnectionFirstEventPrepared, LeReceivedPdu) {
        (
            self.allocation
                .prepare_first_event(self.connection, packet_start),
            self.packet,
        )
    }
}

/// Why an accepted request could not be retired into its originating idle runtime.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralConnectionAcceptedResetCancellationError {
    RuntimeBusy,
    GraphIdentityMismatch,
    ReceiveIdentityMismatch,
}

/// Successful explicit Reset cancellation after the allocation was restored.
///
/// The portable connection has been retired. The copied causal packet remains
/// available only as diagnostic evidence; it carries no peripheral ownership.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retain the causal packet as Reset-cancellation evidence"]
pub(crate) struct PeripheralConnectionAcceptedResetCancelled {
    packet: LeReceivedPdu,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionAcceptedResetCancelled {
    pub(crate) fn into_packet(self) -> LeReceivedPdu {
        self.packet
    }
}

/// Rejected explicit Reset cancellation retaining the complete accepted owner.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the accepted connection and allocation remain affine on rejection"]
pub(crate) struct PeripheralConnectionAcceptedResetCancellationFailure {
    error: PeripheralConnectionAcceptedResetCancellationError,
    accepted: PeripheralConnectionAcceptedRequest,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionAcceptedResetCancellationFailure {
    pub(crate) const fn error(&self) -> PeripheralConnectionAcceptedResetCancellationError {
        self.error
    }

    pub(crate) fn into_accepted(self) -> PeripheralConnectionAcceptedRequest {
        self.accepted
    }
}

/// Connection graph retained while its exact non-scanning RX pool is loaned.
///
/// Connectable advertising and an established peripheral connection share one
/// RX rotation graph. This owner keeps the connection graph affine while that
/// pool is used by the advertising event, and remembers which exact pool may
/// later restore the original runtime allocation.
#[must_use = "the reserved connection graph must be rejoined with its exact receive pool"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PeripheralConnectionRuntimeGraphReserved {
    graph: PeripheralConnectionMemoryGraphCpuOwned,
    receive_identity: NonScanningRxMemoryIdentity,
}

/// Failed graph rejoin retaining both unchanged affine owners.
#[must_use = "the reserved graph and supplied receive pool remain owned"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct PeripheralConnectionRuntimeGraphRejoinFailure {
    _reserved: PeripheralConnectionRuntimeGraphReserved,
    _receive_pool: NonScanningRxMemoryCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionRuntimeGraphRejoinFailure {
    #[cfg(test)]
    pub(crate) fn into_parts(
        self,
    ) -> (
        PeripheralConnectionRuntimeGraphReserved,
        NonScanningRxMemoryCpuOwned,
    ) {
        (self._reserved, self._receive_pool)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionRuntimeGraphReserved {
    /// Restore the original allocation only with the pool split from it.
    ///
    /// A foreign pool is returned together with this reservation, so neither
    /// affine owner is lost and both original pairs can still be reconstructed.
    pub(crate) fn rejoin_receive_pool(
        self,
        receive_pool: NonScanningRxMemoryCpuOwned,
    ) -> Result<PeripheralConnectionRuntimeAllocation, PeripheralConnectionRuntimeGraphRejoinFailure>
    {
        if receive_pool.identity() != self.receive_identity {
            return Err(PeripheralConnectionRuntimeGraphRejoinFailure {
                _reserved: self,
                _receive_pool: receive_pool,
            });
        }

        Ok(PeripheralConnectionRuntimeAllocation::from_claimed_parts(
            self.graph,
            receive_pool,
        ))
    }
}

impl PeripheralConnectionRuntimeAllocation {
    fn from_claimed_parts(
        graph: PeripheralConnectionMemoryGraphCpuOwned,
        receive_pool: NonScanningRxMemoryCpuOwned,
    ) -> Self {
        Self {
            graph,
            receive_pool,
        }
    }

    fn identities(
        &self,
    ) -> (
        PeripheralConnectionMemoryGraphIdentity,
        NonScanningRxMemoryIdentity,
    ) {
        (self.graph.identity(), self.receive_pool.identity())
    }

    /// Whether both graphs retain their pristine allocation topology.
    pub fn is_pristine(&self) -> bool {
        self.graph.has_recovered_scheduler_pool()
            && self.graph.has_empty_receive_queue()
            && self.graph.has_empty_transmit_queue()
            && self.receive_pool.is_initialized()
    }

    /// Reserve the idle connection graph and lend out its exact RX pool.
    ///
    /// This is an ownership-only transition: both returned owners remain
    /// CPU-owned and no descriptor, scheduler list, or MMIO state is changed.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn reserve_graph(
        self,
    ) -> (
        PeripheralConnectionRuntimeGraphReserved,
        NonScanningRxMemoryCpuOwned,
    ) {
        let receive_identity = self.receive_pool.identity();
        (
            PeripheralConnectionRuntimeGraphReserved {
                graph: self.graph,
                receive_identity,
            },
            self.receive_pool,
        )
    }

    /// Join one portable event with the two reviewed S31 identity fields.
    ///
    /// This transition performs controller-SRAM writes only. It cannot publish
    /// a scheduler item or claim that an event has reached hardware.
    pub fn prepare_identity(
        self,
        event: LePeripheralConnectionEventPrepared,
    ) -> PeripheralConnectionIdentityPrepared {
        let request = event.request();
        let identity = PeripheralConnectionIdentity::new(
            request.access_address().value().to_le_bytes(),
            request.crc_initialization().wire_bytes(),
        );
        PeripheralConnectionIdentityPrepared {
            graph: self.graph.prepare_identity(identity),
            receive_pool: self.receive_pool,
            event,
        }
    }

    /// Join the first portable event to the exact accepted packet timestamp.
    ///
    /// The absolute window is derived before descriptor preparation so no
    /// later `now()` sample can replace the causal `CONNECT_IND` observation.
    pub fn prepare_first_event(
        self,
        connection: LePeripheralConnection,
        packet_start: Le1MPacketStartTiming,
    ) -> PeripheralConnectionFirstEventPrepared {
        let event = connection.prepare_event();
        let first_window = packet_start.first_connection_window(event.timing());
        let request = event.request();
        let identity = PeripheralConnectionIdentity::new(
            request.access_address().value().to_le_bytes(),
            request.crc_initialization().wire_bytes(),
        );
        let graph = self
            .graph
            .prepare_identity(identity)
            .attach_receive_pool(self.receive_pool);
        PeripheralConnectionFirstEventPrepared {
            graph,
            event,
            first_window,
        }
    }
}

/// Why the sole connection allocation cannot begin another event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionRuntimeBeginError {
    /// The graph and RX pool are already retained by an affine event owner.
    EventActive,
}

/// Composition-owned reusable allocation for one peripheral connection.
///
/// An empty slot means the exact graph and RX pool are checked out into an
/// event typestate. Dropping that typestate cannot silently restore them.
#[must_use = "the connection runtime retains the sole production allocation"]
pub struct PeripheralConnectionRuntimeResources {
    config: PeripheralConnectionRuntimeConfig,
    graph_identity: PeripheralConnectionMemoryGraphIdentity,
    receive_identity: NonScanningRxMemoryIdentity,
    idle: Option<PeripheralConnectionRuntimeAllocation>,
}

impl PeripheralConnectionRuntimeResources {
    fn from_claimed_parts(
        config: PeripheralConnectionRuntimeConfig,
        graph: PeripheralConnectionMemoryGraphCpuOwned,
        receive_pool: NonScanningRxMemoryCpuOwned,
    ) -> Self {
        let allocation =
            PeripheralConnectionRuntimeAllocation::from_claimed_parts(graph, receive_pool);
        let (graph_identity, receive_identity) = allocation.identities();
        Self {
            config,
            graph_identity,
            receive_identity,
            idle: Some(allocation),
        }
    }

    /// Bind one real statically placed peripheral-connection allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut PeripheralConnectionMemoryGraphStorage,
        receive_storage: &'static mut NonScanningRxMemoryStorage,
        config: PeripheralConnectionRuntimeConfig,
    ) -> Result<Self, PeripheralConnectionRuntimeClaimError> {
        let graph = PeripheralConnectionMemoryGraphStorage::pin_static(storage)
            .map_err(PeripheralConnectionRuntimeClaimError::Graph)?;
        let receive_pool = NonScanningRxMemoryStorage::pin_static(receive_storage)
            .map_err(PeripheralConnectionRuntimeClaimError::Receive)?;
        Ok(Self::from_claimed_parts(config, graph, receive_pool))
    }

    /// Bind one deterministic native model allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut PeripheralConnectionMemoryGraphStorage,
        base: PeripheralConnectionMemoryGraphModelAddress,
        receive_storage: &'static mut NonScanningRxMemoryStorage,
        receive_base: NonScanningRxMemoryModelAddress,
        config: PeripheralConnectionRuntimeConfig,
    ) -> Result<Self, PeripheralConnectionRuntimeClaimError> {
        let graph = PeripheralConnectionMemoryGraphStorage::pin_static_model(storage, base)
            .map_err(PeripheralConnectionRuntimeClaimError::Graph)?;
        let receive_pool =
            NonScanningRxMemoryStorage::pin_static_model(receive_storage, receive_base)
                .map_err(PeripheralConnectionRuntimeClaimError::Receive)?;
        Ok(Self::from_claimed_parts(config, graph, receive_pool))
    }

    /// Immutable physical policy retained by this runtime.
    pub const fn config(&self) -> PeripheralConnectionRuntimeConfig {
        self.config
    }

    /// Physical default transmit power for every connection event.
    pub const fn default_tx_power_dbm(&self) -> PeripheralConnectionDefaultTxPowerDbm {
        self.config.default_tx_power_dbm()
    }

    /// Whether the sole allocation is idle and retains its initial topology.
    pub fn allocation_is_idle(&self) -> bool {
        self.idle
            .as_ref()
            .is_some_and(PeripheralConnectionRuntimeAllocation::is_pristine)
    }

    /// Check out the exact graph and RX pool for one event epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_event(
        &mut self,
    ) -> Result<PeripheralConnectionRuntimeAllocation, PeripheralConnectionRuntimeBeginError> {
        self.idle
            .take()
            .ok_or(PeripheralConnectionRuntimeBeginError::EventActive)
    }

    /// Restore only the exact allocation originally claimed by this runtime.
    pub fn restore_idle(
        &mut self,
        allocation: PeripheralConnectionRuntimeAllocation,
    ) -> Result<(), PeripheralConnectionRuntimeAllocation> {
        let (graph_identity, receive_identity) = allocation.identities();
        if !self.can_restore_allocation(graph_identity, receive_identity) {
            return Err(allocation);
        }
        self.idle = Some(allocation);
        Ok(())
    }

    /// Retire an unlinked connection while preserving foreign/busy owners on rejection.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn retire_active(
        &mut self,
        graph: oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphActiveCpuOwned,
    ) -> Result<(), oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphActiveCpuOwned>
    {
        if !self.can_restore_allocation(graph.identity(), graph.receive_identity()) {
            return Err(graph);
        }
        let (graph, receive_pool) = graph.retire();
        self.idle = Some(PeripheralConnectionRuntimeAllocation {
            graph,
            receive_pool,
        });
        Ok(())
    }

    fn can_restore_allocation(
        &self,
        graph: PeripheralConnectionMemoryGraphIdentity,
        receive: NonScanningRxMemoryIdentity,
    ) -> bool {
        self.idle.is_none() && graph == self.graph_identity && receive == self.receive_identity
    }

    /// Retire one accepted connection only for an explicit pre-publication Reset.
    ///
    /// All rejection checks happen before either the runtime slot or portable
    /// connection changes. Success restores the exact allocation, retires the
    /// portable connection, and returns only the copied causal packet as
    /// diagnostic evidence.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc Reset rejection retains the exact connection allocation and causal packet"
    )]
    pub(crate) fn cancel_accepted_for_reset(
        &mut self,
        accepted: PeripheralConnectionAcceptedRequest,
    ) -> Result<
        PeripheralConnectionAcceptedResetCancelled,
        PeripheralConnectionAcceptedResetCancellationFailure,
    > {
        let (graph_identity, receive_identity) = accepted.allocation.identities();
        let error = if self.idle.is_some() {
            Some(PeripheralConnectionAcceptedResetCancellationError::RuntimeBusy)
        } else if graph_identity != self.graph_identity {
            Some(PeripheralConnectionAcceptedResetCancellationError::GraphIdentityMismatch)
        } else if receive_identity != self.receive_identity {
            Some(PeripheralConnectionAcceptedResetCancellationError::ReceiveIdentityMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(PeripheralConnectionAcceptedResetCancellationFailure { error, accepted });
        }

        let PeripheralConnectionAcceptedRequest {
            allocation,
            connection,
            packet,
        } = accepted;
        self.idle = Some(allocation);
        core::mem::drop(connection);
        Ok(PeripheralConnectionAcceptedResetCancelled { packet })
    }
}

/// Why the complete connection graph plus shared RX pool could not be claimed.
#[derive(Debug)]
pub enum PeripheralConnectionRuntimeClaimError {
    Graph(PeripheralConnectionMemoryGraphBindFailure),
    Receive(NonScanningRxMemoryBindFailure),
}

/// First portable connection event joined to its causal S31 receive timing.
#[must_use = "the timed first connection event must be lowered or cancelled"]
pub struct PeripheralConnectionFirstEventPrepared {
    graph: PeripheralConnectionMemoryGraphReceivePrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
}

impl PeripheralConnectionFirstEventPrepared {
    /// Link Layer event counter, still unadvanced before hardware admission.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Selected first data channel.
    pub const fn channel(&self) -> LeDataChannelIndex {
        self.event.channel()
    }

    #[cfg(test)]
    pub(crate) const fn first_window(&self) -> PeripheralConnectionFirstWindow {
        self.first_window
    }

    /// Width of the first accepted transmit window.
    pub const fn first_window_width_micros(&self) -> u32 {
        self.first_window
            .receive_end
            .image()
            .wrapping_sub(self.first_window.anchor.image())
    }

    /// Whether the bounded non-scanning RX pool is attached before publication.
    pub fn receive_pool_is_initialized(&self) -> bool {
        self.graph.receive_pool_is_initialized()
    }

    /// Project the complete causal event reservation into the retained Controller epoch.
    ///
    /// The common preparation lead and source-owned timing guards precede the
    /// first receive anchor. The end includes both the accepted transmit window
    /// and the complete LE 1M first-event budget; it is not merely the end of
    /// the transmit window. A projection that collapses or exceeds the wrapping
    /// scheduler domain returns this exact unchanged owner.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        dead_code,
        reason = "the next connection scheduler-admission transition consumes this projection"
    )]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc projection failure returns the complete affine event owner"
    )]
    pub(crate) fn project_scheduler_window(
        self,
        epoch: ControllerSchedulerEpoch,
        config: SchedulerSoftwareConfig,
        creation: &crate::controller::time::ControllerTimeSample,
    ) -> Result<PeripheralConnectionFirstEventCandidate, Self> {
        let Some(data_channel) = PeripheralConnectionDataChannel::new(self.event.channel().get())
        else {
            return Err(self);
        };
        let receive_time =
            PeripheralConnectionReceiveTime::from_controller_ticks(creation.raw_ticks());
        let Some(event_span_micros) = self
            .event
            .timing()
            .interval_micros()
            .checked_sub(LE_CONNECTION_COMMON_RESERVE_MICROS)
        else {
            return Err(self);
        };
        let Some(event_span) = PeripheralConnectionEventSpan::new(
            epoch.raw_duration_ticks_for_micros(event_span_micros),
        ) else {
            return Err(self);
        };
        let Some(requested_window) = self.first_window.project_scheduler_window(epoch, config)
        else {
            return Err(self);
        };
        Ok(PeripheralConnectionFirstEventCandidate {
            prepared: self,
            requested_window,
            data_channel,
            receive_time,
            event_span,
        })
    }

    /// Cancel before publication and recover both unchanged owners.
    pub fn cancel(
        self,
    ) -> (
        PeripheralConnectionRuntimeAllocation,
        LePeripheralConnection,
    ) {
        let (graph, receive_pool) = self.graph.cancel();
        (
            PeripheralConnectionRuntimeAllocation::from_claimed_parts(graph.cancel(), receive_pool),
            self.event.cancel(),
        )
    }
}

/// First peripheral event with one epoch-bound raw scheduler candidate.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the projected connection event must enter admission or be cancelled"]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
pub(crate) struct PeripheralConnectionFirstEventCandidate {
    prepared: PeripheralConnectionFirstEventPrepared,
    requested_window: SchedulerRawWindow,
    data_channel: PeripheralConnectionDataChannel,
    receive_time: PeripheralConnectionReceiveTime,
    event_span: PeripheralConnectionEventSpan,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl PeripheralConnectionFirstEventCandidate {
    pub(crate) const fn requested_window(&self) -> SchedulerRawWindow {
        self.requested_window
    }

    pub(crate) const fn event_counter(&self) -> u16 {
        self.prepared.event_counter()
    }

    pub(crate) const fn channel(&self) -> LeDataChannelIndex {
        self.prepared.channel()
    }

    /// Install the reviewed descriptor subset after overlap resolution.
    ///
    /// The resolved common-scheduler window is intentionally accepted here,
    /// rather than during candidate formation, because initial admission may
    /// displace the requested interval. The resulting memory owner still has
    /// no publication operation: direction-finding workspace policy and
    /// hardware admission are mandatory later transitions. The first-event
    /// receive wait and priority are source-owned here, so no caller can pass a
    /// descriptor duration, mode or raw scheduling policy.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc conversion failure returns the complete affine candidate"
    )]
    pub(crate) fn prepare_resolved_event_fields(
        self,
        resolved_window: SchedulerRawWindow,
        raw_sequence_lead: u32,
        default_tx_power: PeripheralConnectionDefaultTxPowerDbm,
    ) -> Result<PeripheralConnectionFirstEventFieldsPrepared, Self> {
        let Some(window) = PeripheralConnectionSchedulerWindow::new(
            resolved_window.start(),
            resolved_window.end(),
        ) else {
            return Err(self);
        };
        let Some(receive_wait) = PeripheralConnectionReceiveWait::new(
            self.prepared.first_window_width_micros(),
            LE_FIRST_EVENT_TIMING_GUARD_MICROS,
        ) else {
            return Err(self);
        };
        let priority = PeripheralConnectionSchedulerPriority::FIRST_EVENT;
        let Self {
            prepared,
            requested_window,
            data_channel,
            receive_time,
            event_span,
        } = self;
        let PeripheralConnectionFirstEventPrepared {
            graph,
            event,
            first_window,
        } = prepared;
        let graph = graph.prepare_reviewed_first_event_fields(
            data_channel,
            receive_time,
            event_span,
            window,
            receive_wait,
            default_tx_power,
            priority,
            raw_sequence_lead,
        );
        Ok(PeripheralConnectionFirstEventFieldsPrepared {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
        })
    }

    pub(crate) fn cancel(
        self,
    ) -> (
        PeripheralConnectionRuntimeAllocation,
        LePeripheralConnection,
    ) {
        self.prepared.cancel()
    }
}

/// Portable first event paired with the reviewed, resolved descriptor subset.
///
/// This state is deliberately CPU-owned and unpublished. It proves that the
/// identity, RX rotation, channel, creation time, power, priority, complete receive
/// wait and resolved event reservation are present, but does not stand in for
/// direction-finding workspace policy or scheduler admission semantics.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the partial connection descriptor must advance or be cancelled"]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
pub(crate) struct PeripheralConnectionFirstEventFieldsPrepared {
    graph: PeripheralConnectionMemoryGraphEventFieldsPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl PeripheralConnectionFirstEventFieldsPrepared {
    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn channel(&self) -> PeripheralConnectionDataChannel {
        self.graph.channel()
    }

    pub(crate) const fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        self.graph.receive_time()
    }

    pub(crate) const fn default_tx_power(&self) -> PeripheralConnectionDefaultTxPowerDbm {
        self.graph.default_tx_power()
    }

    pub(crate) const fn priority(&self) -> PeripheralConnectionSchedulerPriority {
        self.graph.priority()
    }

    pub(crate) const fn requested_window(&self) -> SchedulerRawWindow {
        self.requested_window
    }

    pub(crate) const fn resolved_window(&self) -> SchedulerRawWindow {
        self.resolved_window
    }

    pub(crate) const fn receive_wait(&self) -> PeripheralConnectionReceiveWait {
        self.graph.receive_wait()
    }

    /// Join the powered epoch's controller-global DF workspace.
    ///
    /// The link is opaque here: only the lower memory codec can project it
    /// into the private connection descriptor. The returned event remains
    /// CPU-owned until common scheduler publication is implemented.
    pub(crate) fn install_direction_finding_workspace(
        self,
        workspace: DirectionFindingWorkspaceLink,
    ) -> PeripheralConnectionFirstEventDirectionFindingPrepared {
        PeripheralConnectionFirstEventDirectionFindingPrepared {
            graph: self.graph.install_direction_finding_workspace(workspace),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> (
        PeripheralConnectionRuntimeAllocation,
        LePeripheralConnection,
    ) {
        PeripheralConnectionFirstEventPrepared {
            graph: self.graph.cancel(),
            event: self.event,
            first_window: self.first_window,
        }
        .cancel()
    }
}

/// First peripheral event whose complete reviewed SRAM fields retain the
/// powered epoch's controller-global direction-finding workspace.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the DF-linked connection event must advance or be cancelled"]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
pub(crate) struct PeripheralConnectionFirstEventDirectionFindingPrepared {
    graph: PeripheralConnectionMemoryGraphDirectionFindingPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl PeripheralConnectionFirstEventDirectionFindingPrepared {
    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn channel(&self) -> PeripheralConnectionDataChannel {
        self.graph.channel()
    }

    pub(crate) const fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        self.graph.receive_time()
    }

    pub(crate) const fn requested_window(&self) -> SchedulerRawWindow {
        self.requested_window
    }

    pub(crate) const fn resolved_window(&self) -> SchedulerRawWindow {
        self.resolved_window
    }

    pub(crate) const fn direction_finding_workspace(&self) -> DirectionFindingWorkspaceLink {
        self.graph.direction_finding_workspace()
    }

    /// Detach the exact first-event item from its connection-private free list.
    pub(crate) fn prepare_scheduler_admission(
        self,
    ) -> PeripheralConnectionFirstEventSchedulerAdmissionPrepared {
        PeripheralConnectionFirstEventSchedulerAdmissionPrepared {
            graph: self.graph.prepare_scheduler_admission(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> (
        PeripheralConnectionRuntimeAllocation,
        LePeripheralConnection,
    ) {
        PeripheralConnectionFirstEventFieldsPrepared {
            graph: self.graph.cancel(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
        .cancel()
    }
}

/// DF-linked first event with one item detached for common-list admission.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the detached connection item must be merged or restored"]
#[allow(
    dead_code,
    reason = "the next connection scheduler-publication transition consumes this owner"
)]
pub(crate) struct PeripheralConnectionFirstEventSchedulerAdmissionPrepared {
    graph: PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-publication transition consumes this owner"
)]
impl PeripheralConnectionFirstEventSchedulerAdmissionPrepared {
    pub(crate) const fn scheduler_head(
        &self,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    /// Freeze SRAM initialization before the selector-two publication edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_publication(self) -> PeripheralConnectionFirstEventPublicationPrepared {
        PeripheralConnectionFirstEventPublicationPrepared {
            graph: self.graph.prepare_publication(),
            remainder: PeripheralConnectionFirstEventPublicationRemainder {
                event: self.event,
                first_window: self.first_window,
                requested_window: self.requested_window,
                resolved_window: self.resolved_window,
            },
        }
    }

    pub(crate) fn cancel(self) -> PeripheralConnectionFirstEventDirectionFindingPrepared {
        PeripheralConnectionFirstEventDirectionFindingPrepared {
            graph: self.graph.cancel(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }
}

/// CPU-owned connection graph frozen for selector-two publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the frozen connection graph must be published or retained"]
pub(crate) struct PeripheralConnectionFirstEventPublicationPrepared {
    graph: PeripheralConnectionMemoryGraphPublicationPrepared,
    remainder: PeripheralConnectionFirstEventPublicationRemainder,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionFirstEventPublicationPrepared {
    pub(crate) fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphPublicationPrepared,
        PeripheralConnectionFirstEventPublicationRemainder,
    ) {
        (self.graph, self.remainder)
    }
}

/// Protocol and timing owners retained while the memory graph crosses MMIO.
#[cfg(target_arch = "riscv32")]
#[must_use = "the connection publication remainder must rejoin its exact graph"]
pub(crate) struct PeripheralConnectionFirstEventPublicationRemainder {
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionFirstEventPublicationRemainder {
    pub(crate) fn join_rx_publication(
        self,
        graph: PeripheralConnectionMemoryGraphRxPublished,
    ) -> PeripheralConnectionFirstEventRxPublished {
        PeripheralConnectionFirstEventRxPublished {
            recurring_phase: PeripheralConnectionRecurringPhase::from_nominal_anchor(
                self.first_window.anchor(),
            ),
            graph,
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }
}

/// First connection event whose selector-two RX list is hardware-visible.
#[cfg(target_arch = "riscv32")]
#[must_use = "the RX-published connection event must enter the common scheduler"]
pub(crate) struct PeripheralConnectionFirstEventRxPublished {
    graph: PeripheralConnectionMemoryGraphRxPublished,
    event: LePeripheralConnectionEventPrepared,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionFirstEventRxPublished {
    pub(crate) const fn scheduler_head(
        &self,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) fn into_running(
        self,
        run: &oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareRunCommandPublished,
    ) -> PeripheralConnectionFirstEventRunning {
        PeripheralConnectionFirstEventRunning {
            graph: self.graph.into_running(run),
            event: self.event.into_submitted(),
            _first_window: self.first_window,
            _requested_window: self.requested_window,
            _resolved_window: self.resolved_window,
            recurring_phase: self.recurring_phase,
        }
    }
}

/// Hardware-owned first peripheral connection event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the running connection event must advance through owned completion"]
pub(crate) struct PeripheralConnectionFirstEventRunning {
    graph: PeripheralConnectionMemoryGraphRunning,
    event: LePeripheralConnectionEventInFlight,
    _first_window: PeripheralConnectionFirstWindow,
    _requested_window: SchedulerRawWindow,
    _resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionFirstEventRunning {
    pub(crate) const fn scheduler_item_address(
        &self,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub(crate) fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> PeripheralConnectionFirstEventCompletionObservation {
        let Self {
            graph,
            event,
            _first_window,
            _requested_window,
            _resolved_window,
            recurring_phase,
        } = self;
        match graph.observe_completion(observed) {
            PeripheralConnectionMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => PeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running: Self {
                    graph: running,
                    event,
                    _first_window,
                    _requested_window,
                    _resolved_window,
                    recurring_phase,
                },
                observed,
            },
            PeripheralConnectionMemoryGraphCompletionObservation::StillInFlight(graph) => {
                PeripheralConnectionFirstEventCompletionObservation::StillInFlight(Self {
                    graph,
                    event,
                    _first_window,
                    _requested_window,
                    _resolved_window,
                    recurring_phase,
                })
            }
            PeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(graph) => {
                PeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                    PeripheralConnectionFirstEventCompletionObserved::new(
                        graph,
                        event,
                        _first_window,
                        _requested_window,
                        _resolved_window,
                        recurring_phase,
                    ),
                )
            }
        }
    }
}

/// Exact portable event joined to a CPU-owned, identity-prepared S31 graph.
#[must_use = "the identity-prepared connection event must be retained or cancelled"]
pub struct PeripheralConnectionIdentityPrepared {
    graph: PeripheralConnectionMemoryGraphIdentityPrepared,
    receive_pool: NonScanningRxMemoryCpuOwned,
    event: LePeripheralConnectionEventPrepared,
}

impl PeripheralConnectionIdentityPrepared {
    /// Link Layer event counter which has not advanced yet.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Selected Link Layer data channel for the unsubmitted event.
    pub const fn channel(&self) -> LeDataChannelIndex {
        self.event.channel()
    }

    /// Validated portable timing retained for the future anchor builder.
    pub const fn timing(&self) -> LeConnectionTiming {
        self.event.timing()
    }

    /// Cancel before publication and recover both unchanged protocol state and
    /// the pristine S31 runtime allocation.
    pub fn cancel(
        self,
    ) -> (
        PeripheralConnectionRuntimeAllocation,
        LePeripheralConnection,
    ) {
        (
            PeripheralConnectionRuntimeAllocation::from_claimed_parts(
                self.graph.cancel(),
                self.receive_pool,
            ),
            self.event.cancel(),
        )
    }
}

#[cfg(test)]
mod tests;
