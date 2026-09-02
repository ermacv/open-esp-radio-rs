//! Production ownership for the first ESP32-S31 BLE peripheral connection.
//!
//! The runtime joins a portable LL event to the recovered allocation graph,
//! installs its reviewed Access Address and CRCInit fields and attaches one
//! separately owned static non-scanning RX pool. It cannot publish that partial
//! graph; direction-finding workspace policy and scheduler admission must be
//! closed first.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeConnectionTiming, LeDataChannelIndex,
    LePeripheralConnection, LePeripheralConnectionEventPrepared,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDirectionFindingWorkspaceLink, BluetoothPeripheralConnectionDataChannel,
    BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionIntervalTicks,
    BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared,
    BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared,
    BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    BluetoothPeripheralConnectionReceiveWait, BluetoothPeripheralConnectionSchedulerPriority,
    BluetoothPeripheralConnectionSchedulerWindow,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothNonScanningRxMemoryBindFailure, BluetoothNonScanningRxMemoryCpuOwned,
    BluetoothNonScanningRxMemoryStorage, BluetoothPeripheralConnectionIdentity,
    BluetoothPeripheralConnectionMemoryGraphBindFailure,
    BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
    BluetoothPeripheralConnectionMemoryGraphReceivePrepared,
    BluetoothPeripheralConnectionMemoryGraphStorage,
};
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothNonScanningRxMemoryModelAddress, BluetoothPeripheralConnectionMemoryGraphModelAddress,
};

use crate::BluetoothSchedulerInstant;
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
};

// Source-owned S31 first-event policy. The 5,154-us LE 1M reservation is
// retained by both current and named older S31 controller bodies. The 16-us
// uncertainty is the open NimBLE timing guard. They are backend scheduling
// policy, not portable Link Layer fields and not a vendor aggregate ABI.
const LE_1M_FIRST_EVENT_RESERVATION_MICROS: u32 = 5_154;
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
pub struct BluetoothLe1MPacketStartTiming {
    packet_start: BluetoothSchedulerInstant,
}

impl BluetoothLe1MPacketStartTiming {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_scheduler_micros(micros: u32) -> Self {
        Self {
            packet_start: BluetoothSchedulerInstant::from_image(micros),
        }
    }

    fn first_connection_window(
        self,
        timing: LeConnectionTiming,
    ) -> BluetoothPeripheralConnectionFirstWindow {
        let packet_end = self
            .packet_start
            .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS);
        BluetoothPeripheralConnectionFirstWindow {
            anchor: packet_end.wrapping_add(timing.first_window_start_micros()),
            receive_end: packet_end.wrapping_add(timing.first_window_end_micros()),
            event_end: packet_end
                .wrapping_add(timing.first_window_end_micros())
                .wrapping_add(LE_1M_FIRST_EVENT_RESERVATION_MICROS)
                .wrapping_add(LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS),
        }
    }
}

/// Absolute first receive window and containing event reservation.
///
/// The positions stay private to the S31 scheduler boundary. Portable Link
/// Layer code owns only the relative WinOffset/WinSize semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPeripheralConnectionFirstWindow {
    anchor: BluetoothSchedulerInstant,
    receive_end: BluetoothSchedulerInstant,
    event_end: BluetoothSchedulerInstant,
}

impl BluetoothPeripheralConnectionFirstWindow {
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.anchor
    }

    #[cfg(test)]
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
        self.receive_end
    }

    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        dead_code,
        reason = "the next connection scheduler-admission transition consumes this projection"
    )]
    const fn project_scheduler_window(
        self,
        epoch: BluetoothControllerSchedulerEpoch,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Option<BluetoothSchedulerRawWindow> {
        let scheduler_start = self
            .anchor
            .image()
            .wrapping_sub(config.preparation_lead_micros())
            .wrapping_sub(LE_FIRST_EVENT_TIMING_GUARD_MICROS)
            .wrapping_sub(LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS);
        BluetoothSchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_ticks_for_micros(scheduler_start),
            epoch.raw_ticks_for_micros(self.event_end.image()),
        )
    }
}

/// Composition-owned allocation graph for one future peripheral connection.
#[must_use = "the connection runtime retains the sole production graph"]
pub struct BluetoothPeripheralConnectionRuntimeResources {
    graph: BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    receive_pool: BluetoothNonScanningRxMemoryCpuOwned,
}

impl BluetoothPeripheralConnectionRuntimeResources {
    fn from_claimed_parts(
        graph: BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        receive_pool: BluetoothNonScanningRxMemoryCpuOwned,
    ) -> Self {
        Self {
            graph,
            receive_pool,
        }
    }

    /// Bind one real statically placed peripheral-connection allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        receive_storage: &'static mut BluetoothNonScanningRxMemoryStorage,
    ) -> Result<Self, BluetoothPeripheralConnectionRuntimeClaimError> {
        let graph = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static(storage)
            .map_err(BluetoothPeripheralConnectionRuntimeClaimError::Graph)?;
        let receive_pool = BluetoothNonScanningRxMemoryStorage::pin_static(receive_storage)
            .map_err(BluetoothPeripheralConnectionRuntimeClaimError::Receive)?;
        Ok(Self::from_claimed_parts(graph, receive_pool))
    }

    /// Bind one deterministic native model allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        base: BluetoothPeripheralConnectionMemoryGraphModelAddress,
        receive_storage: &'static mut BluetoothNonScanningRxMemoryStorage,
        receive_base: BluetoothNonScanningRxMemoryModelAddress,
    ) -> Result<Self, BluetoothPeripheralConnectionRuntimeClaimError> {
        let graph =
            BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage, base)
                .map_err(BluetoothPeripheralConnectionRuntimeClaimError::Graph)?;
        let receive_pool =
            BluetoothNonScanningRxMemoryStorage::pin_static_model(receive_storage, receive_base)
                .map_err(BluetoothPeripheralConnectionRuntimeClaimError::Receive)?;
        Ok(Self::from_claimed_parts(graph, receive_pool))
    }

    /// Whether the retained allocation still has its initial queue topology.
    pub fn allocation_is_idle(&self) -> bool {
        self.graph.has_recovered_scheduler_pool()
            && self.graph.has_empty_receive_queue()
            && self.graph.has_empty_transmit_queue()
            && self.receive_pool.is_initialized()
    }

    /// Join one portable event with the two reviewed S31 identity fields.
    ///
    /// This transition performs controller-SRAM writes only. It cannot publish
    /// a scheduler item or claim that an event has reached hardware.
    pub fn prepare_identity(
        self,
        event: LePeripheralConnectionEventPrepared,
    ) -> BluetoothPeripheralConnectionIdentityPrepared {
        let request = event.request();
        let identity = BluetoothPeripheralConnectionIdentity::new(
            request.access_address().value().to_le_bytes(),
            request.crc_initialization().wire_bytes(),
        );
        BluetoothPeripheralConnectionIdentityPrepared {
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
        packet_start: BluetoothLe1MPacketStartTiming,
    ) -> BluetoothPeripheralConnectionFirstEventPrepared {
        let event = connection.prepare_event();
        let first_window = packet_start.first_connection_window(event.timing());
        let request = event.request();
        let identity = BluetoothPeripheralConnectionIdentity::new(
            request.access_address().value().to_le_bytes(),
            request.crc_initialization().wire_bytes(),
        );
        let graph = self
            .graph
            .prepare_identity(identity)
            .attach_receive_pool(self.receive_pool);
        BluetoothPeripheralConnectionFirstEventPrepared {
            graph,
            event,
            first_window,
        }
    }
}

/// Why the complete connection graph plus shared RX pool could not be claimed.
#[derive(Debug)]
pub enum BluetoothPeripheralConnectionRuntimeClaimError {
    Graph(BluetoothPeripheralConnectionMemoryGraphBindFailure),
    Receive(BluetoothNonScanningRxMemoryBindFailure),
}

/// First portable connection event joined to its causal S31 receive timing.
#[must_use = "the timed first connection event must be lowered or cancelled"]
pub struct BluetoothPeripheralConnectionFirstEventPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphReceivePrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: BluetoothPeripheralConnectionFirstWindow,
}

impl BluetoothPeripheralConnectionFirstEventPrepared {
    /// Link Layer event counter, still unadvanced before hardware admission.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Selected first data channel.
    pub const fn channel(&self) -> LeDataChannelIndex {
        self.event.channel()
    }

    #[cfg(test)]
    pub(crate) const fn first_window(&self) -> BluetoothPeripheralConnectionFirstWindow {
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
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc projection failure returns the complete affine event owner"
    )]
    pub(crate) fn project_scheduler_window(
        self,
        epoch: BluetoothControllerSchedulerEpoch,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<BluetoothPeripheralConnectionFirstEventCandidate, Self> {
        let Some(data_channel) =
            BluetoothPeripheralConnectionDataChannel::new(self.event.channel().get())
        else {
            return Err(self);
        };
        let interval_ticks =
            epoch.raw_duration_ticks_for_micros(self.event.timing().interval_micros());
        let Some(interval) = BluetoothPeripheralConnectionIntervalTicks::new(interval_ticks) else {
            return Err(self);
        };
        let Some(requested_window) = self.first_window.project_scheduler_window(epoch, config)
        else {
            return Err(self);
        };
        Ok(BluetoothPeripheralConnectionFirstEventCandidate {
            prepared: self,
            requested_window,
            data_channel,
            interval,
        })
    }

    /// Cancel before publication and recover both unchanged owners.
    pub fn cancel(
        self,
    ) -> (
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        let (graph, receive_pool) = self.graph.cancel();
        (
            BluetoothPeripheralConnectionRuntimeResources::from_claimed_parts(
                graph.cancel(),
                receive_pool,
            ),
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
pub(crate) struct BluetoothPeripheralConnectionFirstEventCandidate {
    prepared: BluetoothPeripheralConnectionFirstEventPrepared,
    requested_window: BluetoothSchedulerRawWindow,
    data_channel: BluetoothPeripheralConnectionDataChannel,
    interval: BluetoothPeripheralConnectionIntervalTicks,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl BluetoothPeripheralConnectionFirstEventCandidate {
    pub(crate) const fn requested_window(&self) -> BluetoothSchedulerRawWindow {
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
        resolved_window: BluetoothSchedulerRawWindow,
        default_tx_power: BluetoothPeripheralConnectionDefaultTxPowerDbm,
    ) -> Result<BluetoothPeripheralConnectionFirstEventFieldsPrepared, Self> {
        let Some(window) = BluetoothPeripheralConnectionSchedulerWindow::new(
            resolved_window.start(),
            resolved_window.end(),
        ) else {
            return Err(self);
        };
        let Some(receive_wait) = BluetoothPeripheralConnectionReceiveWait::new(
            self.prepared.first_window_width_micros(),
            LE_FIRST_EVENT_TIMING_GUARD_MICROS,
        ) else {
            return Err(self);
        };
        let priority = BluetoothPeripheralConnectionSchedulerPriority::FIRST_EVENT;
        let Self {
            prepared,
            requested_window,
            data_channel,
            interval,
        } = self;
        let BluetoothPeripheralConnectionFirstEventPrepared {
            graph,
            event,
            first_window,
        } = prepared;
        let graph = graph.prepare_reviewed_first_event_fields(
            data_channel,
            interval,
            window,
            receive_wait,
            default_tx_power,
            priority,
        );
        Ok(BluetoothPeripheralConnectionFirstEventFieldsPrepared {
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
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        self.prepared.cancel()
    }
}

/// Portable first event paired with the reviewed, resolved descriptor subset.
///
/// This state is deliberately CPU-owned and unpublished. It proves that the
/// identity, RX rotation, channel, interval, power, priority, complete receive
/// wait and resolved event reservation are present, but does not stand in for
/// direction-finding workspace policy or scheduler admission semantics.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the partial connection descriptor must advance or be cancelled"]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
pub(crate) struct BluetoothPeripheralConnectionFirstEventFieldsPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl BluetoothPeripheralConnectionFirstEventFieldsPrepared {
    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn channel(&self) -> BluetoothPeripheralConnectionDataChannel {
        self.graph.channel()
    }

    pub(crate) const fn interval(&self) -> BluetoothPeripheralConnectionIntervalTicks {
        self.graph.interval()
    }

    pub(crate) const fn default_tx_power(&self) -> BluetoothPeripheralConnectionDefaultTxPowerDbm {
        self.graph.default_tx_power()
    }

    pub(crate) const fn priority(&self) -> BluetoothPeripheralConnectionSchedulerPriority {
        self.graph.priority()
    }

    pub(crate) const fn requested_window(&self) -> BluetoothSchedulerRawWindow {
        self.requested_window
    }

    pub(crate) const fn resolved_window(&self) -> BluetoothSchedulerRawWindow {
        self.resolved_window
    }

    pub(crate) const fn receive_wait(&self) -> BluetoothPeripheralConnectionReceiveWait {
        self.graph.receive_wait()
    }

    /// Join the powered epoch's controller-global DF workspace.
    ///
    /// The link is opaque here: only the lower memory codec can project it
    /// into the private connection descriptor. The returned event remains
    /// CPU-owned until common scheduler publication is implemented.
    pub(crate) fn install_direction_finding_workspace(
        self,
        workspace: BluetoothDirectionFindingWorkspaceLink,
    ) -> BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
        BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
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
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        BluetoothPeripheralConnectionFirstEventPrepared {
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
pub(crate) struct BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-admission transition consumes this owner"
)]
impl BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn channel(&self) -> BluetoothPeripheralConnectionDataChannel {
        self.graph.channel()
    }

    pub(crate) const fn interval(&self) -> BluetoothPeripheralConnectionIntervalTicks {
        self.graph.interval()
    }

    pub(crate) const fn requested_window(&self) -> BluetoothSchedulerRawWindow {
        self.requested_window
    }

    pub(crate) const fn resolved_window(&self) -> BluetoothSchedulerRawWindow {
        self.resolved_window
    }

    pub(crate) const fn direction_finding_workspace(
        &self,
    ) -> BluetoothDirectionFindingWorkspaceLink {
        self.graph.direction_finding_workspace()
    }

    /// Detach the exact first-event item from its connection-private free list.
    pub(crate) fn prepare_scheduler_admission(
        self,
    ) -> BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared {
        BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared {
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
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        BluetoothPeripheralConnectionFirstEventFieldsPrepared {
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
pub(crate) struct BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    event: LePeripheralConnectionEventPrepared,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[allow(
    dead_code,
    reason = "the next connection scheduler-publication transition consumes this owner"
)]
impl BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared {
    pub(crate) const fn scheduler_head(
        &self,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    pub(crate) fn cancel(self) -> BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
        BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared {
            graph: self.graph.cancel(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }
}

/// Exact portable event joined to a CPU-owned, identity-prepared S31 graph.
#[must_use = "the identity-prepared connection event must be retained or cancelled"]
pub struct BluetoothPeripheralConnectionIdentityPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
    receive_pool: BluetoothNonScanningRxMemoryCpuOwned,
    event: LePeripheralConnectionEventPrepared,
}

impl BluetoothPeripheralConnectionIdentityPrepared {
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
        BluetoothPeripheralConnectionRuntimeResources,
        LePeripheralConnection,
    ) {
        (
            BluetoothPeripheralConnectionRuntimeResources::from_claimed_parts(
                self.graph.cancel(),
                self.receive_pool,
            ),
            self.event.cancel(),
        )
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::connection::{
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LEGACY_CONNECT_IND_PAYLOAD_BYTES,
        LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest, LePeripheralConnection,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDirectionFindingWorkspaceStorage,
        BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
        BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionIntervalTicks,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
        BluetoothPeripheralConnectionSchedulerPriority,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{BluetoothLe1MPacketStartTiming, BluetoothPeripheralConnectionRuntimeResources};
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
        BluetoothSchedulerRawWindow, BluetoothSchedulerSoftwareConfig,
    };

    fn runtime(graph_base: u32) -> BluetoothPeripheralConnectionRuntimeResources {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(graph_base)
            .expect("the model graph base is a controller SRAM address");
        let receive_base =
            BluetoothNonScanningRxMemoryModelAddress::new(graph_base.wrapping_add(0x1000))
                .expect("the model receive base is a controller SRAM address");
        BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
            storage,
            base,
            receive_storage,
            receive_base,
        )
        .expect("the model graph and receive pool fit controller SRAM")
    }

    #[test]
    fn claimed_runtime_retains_the_idle_allocation() {
        let runtime = runtime(0x2f00_1000);

        assert!(runtime.allocation_is_idle());
    }

    #[test]
    fn portable_event_can_prepare_identity_and_cancel_losslessly() {
        let runtime = runtime(0x2f00_2000);
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let connection = LePeripheralConnection::from_request(request);

        let prepared = runtime.prepare_identity(connection.prepare_event());
        assert_eq!(prepared.event_counter(), 0);
        assert!(prepared.channel().get() < 37);
        assert_eq!(prepared.timing().interval_micros(), 30_000);

        let (runtime, connection) = prepared.cancel();
        assert!(runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn first_event_uses_the_received_packet_start_for_its_absolute_window() {
        let runtime = runtime(0x2f00_3000);
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let connection = LePeripheralConnection::from_request(request);

        let prepared = runtime.prepare_first_event(
            connection,
            BluetoothLe1MPacketStartTiming::from_scheduler_micros(u32::MAX - 100),
        );
        assert!(prepared.receive_pool_is_initialized());
        let window = prepared.first_window();
        assert_eq!(
            window.anchor().image(),
            (u32::MAX - 100)
                .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
                .wrapping_add(request.timing().first_window_start_micros())
        );
        assert_eq!(
            window.end().image().wrapping_sub(window.anchor().image()),
            request
                .timing()
                .first_window_end_micros()
                .wrapping_sub(request.timing().first_window_start_micros())
        );

        let (runtime, connection) = prepared.cancel();
        assert!(runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn first_event_projects_one_preparation_window_without_losing_ownership() {
        let runtime = runtime(0x2f00_5000);
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let expected_channel = LePeripheralConnection::from_request(request)
            .prepare_event()
            .channel();
        let packet_start_micros = 10_000;
        let prepared = runtime.prepare_first_event(
            LePeripheralConnection::from_request(request),
            BluetoothLe1MPacketStartTiming::from_scheduler_micros(packet_start_micros),
        );
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            9_000,
            scale,
        );
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
        let candidate = match prepared.project_scheduler_window(epoch, config) {
            Ok(candidate) => candidate,
            Err(_) => panic!("the accepted first window has a non-empty raw projection"),
        };
        let anchor = packet_start_micros
            .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
            .wrapping_add(request.timing().first_window_start_micros());

        assert_eq!(candidate.event_counter(), 0);
        assert_eq!(candidate.channel(), expected_channel);
        assert_eq!(
            candidate.requested_window().start(),
            epoch.raw_ticks_for_micros(
                anchor
                    .wrapping_sub(config.preparation_lead_micros())
                    .wrapping_sub(super::LE_FIRST_EVENT_TIMING_GUARD_MICROS)
                    .wrapping_sub(super::LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS)
            )
        );
        assert_eq!(
            candidate.requested_window().end(),
            epoch.raw_ticks_for_micros(
                packet_start_micros
                    .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
                    .wrapping_add(request.timing().first_window_end_micros())
                    .wrapping_add(super::LE_1M_FIRST_EVENT_RESERVATION_MICROS)
                    .wrapping_add(super::LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS)
            )
        );

        let (runtime, connection) = candidate.cancel();
        assert!(runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn resolved_connection_fields_remain_affine_and_cancel_losslessly() {
        let runtime = runtime(0x2f00_7000);
        let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
        let expected_channel = LePeripheralConnection::from_request(request)
            .prepare_event()
            .channel();
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(300),
            20_000,
            scale,
        );
        let candidate = runtime
            .prepare_first_event(
                LePeripheralConnection::from_request(request),
                BluetoothLe1MPacketStartTiming::from_scheduler_micros(21_000),
            )
            .project_scheduler_window(
                epoch,
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .unwrap_or_else(|_| panic!("the first connection window projects"));
        let requested = candidate.requested_window();
        let resolved = BluetoothSchedulerRawWindow::from_projected_scheduler_window(
            requested.end(),
            requested.end().wrapping_add(requested.duration()),
        )
        .expect("the displaced window retains the accepted duration");
        let default_tx_power = BluetoothPeripheralConnectionDefaultTxPowerDbm::new(-4);
        let priority = BluetoothPeripheralConnectionSchedulerPriority::FIRST_EVENT;

        let prepared = candidate
            .prepare_resolved_event_fields(resolved, default_tx_power)
            .unwrap_or_else(|_| panic!("a resolved scheduler window remains non-empty"));

        assert_eq!(prepared.event_counter(), 0);
        assert_eq!(prepared.channel().index(), expected_channel.get());
        assert_eq!(prepared.default_tx_power(), default_tx_power);
        assert_eq!(prepared.priority(), priority);
        assert_eq!(prepared.requested_window(), requested);
        assert_eq!(prepared.resolved_window(), resolved);
        assert_eq!(
            prepared.receive_wait().transmit_window_micros(),
            request
                .timing()
                .first_window_end_micros()
                .wrapping_sub(request.timing().first_window_start_micros())
        );
        assert_eq!(
            prepared.receive_wait().timing_guard_micros(),
            super::LE_FIRST_EVENT_TIMING_GUARD_MICROS
        );
        assert_eq!(
            prepared.interval(),
            BluetoothPeripheralConnectionIntervalTicks::new(
                epoch.raw_duration_ticks_for_micros(request.timing().interval_micros())
            )
            .expect("a validated LE connection interval projects to non-zero ticks")
        );

        let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothDirectionFindingWorkspaceStorage::new(),
        ));
        let workspace_base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_6000)
            .expect("the model workspace base is a controller SRAM address");
        let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
            workspace_storage,
            workspace_base,
        )
        .expect("the complete workspace fits controller SRAM");
        let workspace_link = workspace.binding().link();
        let prepared = prepared.install_direction_finding_workspace(workspace_link);

        assert_eq!(prepared.event_counter(), 0);
        assert_eq!(prepared.channel().index(), expected_channel.get());
        assert_eq!(prepared.requested_window(), requested);
        assert_eq!(prepared.resolved_window(), resolved);
        assert_eq!(prepared.direction_finding_workspace(), workspace_link);

        let (runtime, connection) = prepared.cancel();
        assert!(runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
    }

    fn connection_request() -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0x25;
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        pdu[35] = 5;
        pdu
    }
}
