//! Executor-neutral bookends for one reusable DTM memory-graph session.
//!
//! This layer deliberately does not model controller execution. It provides
//! only the honest bookends around that missing session pump: an idle graph can
//! begin a fresh CPU-owned epoch, and an already ended test retains its graph
//! and report until response publication. The allocation configuration is
//! inseparable from the graph binding throughout both bookends.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphBindFailure, BluetoothDtmMemoryGraphCpuOwned,
    BluetoothDtmMemoryGraphIdentity, BluetoothDtmMemoryGraphReclaimed,
    BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
};

use crate::BluetoothDtmDefaultTxPowerDbm;
#[cfg(any(target_arch = "riscv32", test))]
use crate::dtm_event_prepare::{BluetoothDtmTestEndReport, BluetoothDtmTestEndedCpuOwned};

/// Composition-owned immutable inputs for one reusable DTM runtime.
///
/// Allocation inputs are consumed by the static graph claim and retained by
/// that graph for every later reinitialization. Default transmit power remains
/// available to the chip task service so callers cannot inject a different
/// link-state policy for each command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRuntimeConfig {
    scheduler_allocation: BluetoothDtmSchedulerAllocationConfig,
    default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm,
}

impl BluetoothDtmRuntimeConfig {
    /// Bind graph allocation inputs and physical default power once.
    pub const fn new(
        scheduler_allocation: BluetoothDtmSchedulerAllocationConfig,
        default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm,
    ) -> Self {
        Self {
            scheduler_allocation,
            default_tx_power_dbm,
        }
    }

    /// Allocation configuration used to bind and reinitialize the graph.
    pub const fn scheduler_allocation_config(self) -> BluetoothDtmSchedulerAllocationConfig {
        self.scheduler_allocation
    }

    /// Physical default transmit-power request for link-state reset.
    pub const fn default_tx_power_dbm(self) -> BluetoothDtmDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }
}

/// Why the reusable graph cannot begin another CPU-owned session epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRuntimeSessionBeginError {
    /// The sole graph is already checked out into an active affine typestate.
    SessionActive,
}

/// Runtime configuration and the sole reusable DTM graph.
///
/// The empty slot is durable state, not absence of production allocation: it
/// means the graph is checked out into an active session typestate. Dropping
/// that typestate cannot silently restore the slot.
#[must_use = "the DTM runtime retains the sole production graph"]
pub struct BluetoothDtmRuntimeResources {
    config: BluetoothDtmRuntimeConfig,
    graph_identity: BluetoothDtmMemoryGraphIdentity,
    idle: Option<BluetoothDtmSessionIdle>,
}

impl BluetoothDtmRuntimeResources {
    fn from_claimed_graph(
        default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm,
        graph: BluetoothDtmMemoryGraphCpuOwned,
    ) -> Self {
        let binding = graph.binding();
        let config =
            BluetoothDtmRuntimeConfig::new(binding.allocation_config(), default_tx_power_dbm);
        let graph_identity = binding.identity();
        Self {
            config,
            graph_identity,
            idle: Some(BluetoothDtmSessionIdle::new(graph)),
        }
    }

    /// Bind one real statically placed graph to its composition-owned policy.
    ///
    /// The factory itself supplies the retained allocation configuration to
    /// the lower binding operation. A caller therefore cannot pair a graph
    /// initialized from one allocation policy with a different runtime policy.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut BluetoothDtmMemoryGraphStorage,
        config: BluetoothDtmRuntimeConfig,
    ) -> Result<Self, BluetoothDtmMemoryGraphBindFailure> {
        let graph = BluetoothDtmMemoryGraphStorage::pin_static(
            storage,
            config.scheduler_allocation_config(),
        )?;
        Ok(Self::from_claimed_graph(
            config.default_tx_power_dbm(),
            graph,
        ))
    }

    /// Bind one native model graph to its composition-owned policy.
    ///
    /// As in the real-address factory, the graph and retained configuration
    /// are produced by one operation rather than accepted as independently
    /// forgeable inputs.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothDtmMemoryGraphStorage,
        base: BluetoothDtmMemoryGraphModelAddress,
        config: BluetoothDtmRuntimeConfig,
    ) -> Result<Self, BluetoothDtmMemoryGraphBindFailure> {
        let graph = BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            base,
            config.scheduler_allocation_config(),
        )?;
        Ok(Self::from_claimed_graph(
            config.default_tx_power_dbm(),
            graph,
        ))
    }

    /// Immutable configuration retained for this exact graph runtime.
    pub const fn config(&self) -> BluetoothDtmRuntimeConfig {
        self.config
    }

    /// Physical default transmit-power request retained by this runtime.
    pub const fn default_tx_power_dbm(&self) -> BluetoothDtmDefaultTxPowerDbm {
        self.config.default_tx_power_dbm()
    }

    /// Whether the sole graph is currently retained at the idle boundary.
    pub const fn session_is_idle(&self) -> bool {
        self.idle.is_some()
    }

    /// Check out the sole graph and begin one fresh allocation epoch.
    ///
    /// Failure leaves the already-active graph untouched. Success leaves this
    /// runtime slot empty until a cancelled or fully stopped session returns
    /// its exact idle owner through [`Self::restore_idle`].
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_session_epoch(
        &mut self,
    ) -> Result<BluetoothDtmSessionGraphReady, BluetoothDtmRuntimeSessionBeginError> {
        let idle = self
            .idle
            .take()
            .ok_or(BluetoothDtmRuntimeSessionBeginError::SessionActive)?;
        Ok(idle.begin_epoch())
    }

    /// Restore one cancelled or fully stopped session to the vacant slot.
    ///
    /// Rejection returns the supplied graph unchanged. It preserves either the
    /// graph already retained by this runtime or its vacant state when the
    /// supplied graph belongs to another pinned storage object.
    pub fn restore_idle(
        &mut self,
        idle: BluetoothDtmSessionIdle,
    ) -> Result<(), BluetoothDtmSessionIdle> {
        if self.idle.is_some() || idle.graph.binding().identity() != self.graph_identity {
            return Err(idle);
        }
        self.idle = Some(idle);
        Ok(())
    }
}

/// Idle DTM session retaining one reclaimed static memory graph.
///
/// Construction ends the allocation epoch represented by the supplied CPU
/// owner. A new epoch can start only by consuming this value.
#[must_use = "the idle session retains the sole reusable DTM graph"]
pub struct BluetoothDtmSessionIdle {
    graph: BluetoothDtmMemoryGraphReclaimed,
}

impl BluetoothDtmSessionIdle {
    /// Capture a newly allocated or otherwise ordinary CPU-owned graph.
    pub fn new(graph: BluetoothDtmMemoryGraphCpuOwned) -> Self {
        Self {
            graph: graph.into_reclaimed(),
        }
    }

    /// Reinitialize the retained graph and begin one fresh allocation epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_epoch(self) -> BluetoothDtmSessionGraphReady {
        BluetoothDtmSessionGraphReady {
            graph: self.graph.reinitialize(),
        }
    }
}

/// Fresh CPU-owned graph at the start of one DTM allocation epoch.
#[must_use = "the fresh DTM graph must advance or return to the idle session"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmSessionGraphReady {
    graph: BluetoothDtmMemoryGraphCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmSessionGraphReady {
    /// Release the fresh CPU owner to the concrete lower session pump.
    ///
    /// No session continuity is claimed beyond this edge. The future pump must
    /// retain the exact lower typestate until it can produce a Test End owner.
    pub fn into_graph(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.graph
    }

    /// Cancel this CPU-only epoch and return its graph to idle retention.
    #[cfg(test)]
    pub fn cancel(self) -> BluetoothDtmSessionIdle {
        BluetoothDtmSessionIdle {
            graph: self.graph.into_reclaimed(),
        }
    }
}

/// Ended DTM session retaining its graph and report during response backpressure.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the Test End response must be published before the graph can be reused"]
pub(crate) struct BluetoothDtmSessionStopping {
    ended: BluetoothDtmTestEndedCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmSessionStopping {
    /// Retain one lower Test End owner while its response is backpressured.
    ///
    /// The chip stopping runner constructs this bookend only after its exact
    /// active graph reached the CPU-owned completion boundary.
    pub(crate) fn new(ended: BluetoothDtmTestEndedCpuOwned) -> Self {
        Self { ended }
    }

    /// Borrow the stable role-specific report for response serialization.
    pub(crate) const fn report(&self) -> BluetoothDtmTestEndReport {
        self.ended.report()
    }

    /// Release the reclaimed graph only after the response was published.
    pub(crate) fn response_published(self) -> BluetoothDtmSessionIdle {
        BluetoothDtmSessionIdle {
            graph: self.ended.into_reclaimed_graph(),
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BLUETOOTH_DTM_MAX_PACKET_CAPACITY, BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
        BluetoothDtmSchedulerAllocationConfig,
    };

    use super::{
        BluetoothDtmRuntimeConfig, BluetoothDtmRuntimeResources,
        BluetoothDtmRuntimeSessionBeginError, BluetoothDtmSessionIdle,
    };
    use crate::BluetoothDtmDefaultTxPowerDbm;

    const fn runtime_config() -> BluetoothDtmRuntimeConfig {
        BluetoothDtmRuntimeConfig::new(
            BluetoothDtmSchedulerAllocationConfig::new(1, 2, 5, 1),
            BluetoothDtmDefaultTxPowerDbm::new(7),
        )
    }

    fn graph_at(address: u32) -> BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(address)
            .expect("model address uses the controller SRAM syntax");
        let config = BluetoothDtmSchedulerAllocationConfig::new(1, 2, 5, 1);
        BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config)
            .expect("model graph fits in controller SRAM")
    }

    fn graph() -> BluetoothDtmMemoryGraphCpuOwned {
        graph_at(0x2f00_6800)
    }

    #[test]
    fn idle_reinitializes_the_same_graph_for_each_cpu_owned_epoch() {
        let payload = [0xa5; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
        let prepared = BluetoothDtmSessionIdle::new(graph())
            .begin_epoch()
            .into_graph()
            .prepare_tx_packet(3, 5, &payload);

        assert_eq!(prepared.pattern_selector(), 3);
        assert_eq!(prepared.payload_length(), 5);

        let prepared = BluetoothDtmSessionIdle::new(prepared.discard_packet_readiness())
            .begin_epoch()
            .into_graph()
            .prepare_tx_packet(6, 2, &payload);

        assert_eq!(prepared.pattern_selector(), 6);
        assert_eq!(prepared.payload_length(), 2);

        let _idle = BluetoothDtmSessionIdle::new(prepared.discard_packet_readiness());
    }

    #[test]
    fn cancelling_graph_ready_returns_the_graph_to_idle_retention() {
        let idle = BluetoothDtmSessionIdle::new(graph()).begin_epoch().cancel();
        let _ready = idle.begin_epoch();
    }

    #[test]
    fn runtime_checkout_is_exclusive_and_cancelled_session_restores_it() {
        let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
            runtime_config().default_tx_power_dbm(),
            graph(),
        );
        assert!(runtime.session_is_idle());
        assert_eq!(runtime.config(), runtime_config());
        assert_eq!(runtime.default_tx_power_dbm().dbm(), 7);

        let ready = runtime
            .begin_session_epoch()
            .expect("the retained idle graph begins one epoch");
        assert!(!runtime.session_is_idle());
        assert!(matches!(
            runtime.begin_session_epoch(),
            Err(BluetoothDtmRuntimeSessionBeginError::SessionActive)
        ));

        runtime
            .restore_idle(ready.cancel())
            .unwrap_or_else(|_| panic!("the vacant runtime slot accepts its cancelled session"));
        assert!(runtime.session_is_idle());
        let _next = runtime
            .begin_session_epoch()
            .expect("the restored graph begins another epoch");
    }

    #[test]
    fn vacant_runtime_rejects_a_foreign_graph_without_losing_either_owner() {
        let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
            runtime_config().default_tx_power_dbm(),
            graph_at(0x2f00_6800),
        );
        let own = runtime
            .begin_session_epoch()
            .expect("the retained graph leaves its runtime slot vacant");

        let mut foreign_runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
            runtime_config().default_tx_power_dbm(),
            graph_at(0x2f00_7800),
        );
        let foreign = foreign_runtime
            .begin_session_epoch()
            .expect("the foreign graph also leaves its own slot vacant")
            .cancel();

        let foreign = runtime
            .restore_idle(foreign)
            .expect_err("a vacant runtime still rejects another bound graph");
        assert!(!runtime.session_is_idle());
        runtime
            .restore_idle(own.cancel())
            .unwrap_or_else(|_| panic!("the exact graph returns to its vacant runtime"));
        foreign_runtime
            .restore_idle(foreign)
            .unwrap_or_else(|_| panic!("the rejected graph remains returnable to its owner"));
        assert!(runtime.session_is_idle());
        assert!(foreign_runtime.session_is_idle());
    }

    #[test]
    fn vacant_runtime_rejects_distinct_storage_with_the_same_model_binding() {
        let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
            runtime_config().default_tx_power_dbm(),
            graph_at(0x2f00_6800),
        );
        let own = runtime
            .begin_session_epoch()
            .expect("the retained graph leaves its runtime slot vacant");

        let mut foreign_runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
            runtime_config().default_tx_power_dbm(),
            graph_at(0x2f00_6800),
        );
        let foreign = foreign_runtime
            .begin_session_epoch()
            .expect("the distinct storage has the same modeled range and configuration")
            .cancel();

        let foreign = runtime
            .restore_idle(foreign)
            .expect_err("modeled range equality cannot substitute for exact storage identity");
        assert!(!runtime.session_is_idle());
        runtime
            .restore_idle(own.cancel())
            .unwrap_or_else(|_| panic!("the exact graph returns to its vacant runtime"));
        foreign_runtime
            .restore_idle(foreign)
            .unwrap_or_else(|_| panic!("the rejected graph remains returnable to its owner"));
        assert!(runtime.session_is_idle());
        assert!(foreign_runtime.session_is_idle());
    }
}
