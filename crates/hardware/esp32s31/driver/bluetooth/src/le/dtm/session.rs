//! Reusable DTM memory-graph epochs and terminal response retention.
//!
//! Controller execution lives in the first-event, active-session and stopping
//! runners. This layer owns their reusable bookends: an idle graph begins a
//! fresh CPU-owned epoch, and an ended test retains its graph and report until
//! exact-once response publication. The allocation configuration stays
//! inseparable from the graph binding throughout every epoch.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "riscv32"))]
use oer_esp32s31_bluetooth_memory::DtmMemoryGraphModelAddress;

#[cfg(any(target_arch = "riscv32", test))]
use crate::le::dtm::event::prepare::{DtmQuiescedCpuOwned, DtmTestEndReport, DtmTestEndedCpuOwned};

use crate::le::dtm::DtmDefaultTxPowerDbm;

use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphBindFailure, DtmMemoryGraphCpuOwned, DtmMemoryGraphIdentity,
    DtmMemoryGraphReclaimed, DtmMemoryGraphStorage, DtmSchedulerAllocationConfig,
};

/// Composition-owned immutable inputs for one reusable DTM runtime.
///
/// Allocation inputs are consumed by the static graph claim and retained by
/// that graph for every later reinitialization. Default transmit power remains
/// available to the chip task service so callers cannot inject a different
/// link-state policy for each command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRuntimeConfig {
    scheduler_allocation: DtmSchedulerAllocationConfig,
    default_tx_power_dbm: DtmDefaultTxPowerDbm,
}

impl DtmRuntimeConfig {
    /// Bind graph allocation inputs and physical default power once.
    pub const fn new(
        scheduler_allocation: DtmSchedulerAllocationConfig,
        default_tx_power_dbm: DtmDefaultTxPowerDbm,
    ) -> Self {
        Self {
            scheduler_allocation,
            default_tx_power_dbm,
        }
    }

    /// Allocation configuration used to bind and reinitialize the graph.
    pub const fn scheduler_allocation_config(self) -> DtmSchedulerAllocationConfig {
        self.scheduler_allocation
    }

    /// Physical default transmit-power request for link-state reset.
    pub const fn default_tx_power_dbm(self) -> DtmDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }
}

/// Why the reusable graph cannot begin another CPU-owned session epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRuntimeSessionBeginError {
    /// The sole graph is already checked out into an active affine typestate.
    SessionActive,
}

/// Runtime configuration and the sole reusable DTM graph.
///
/// The empty slot is durable state, not absence of production allocation: it
/// means the graph is checked out into an active session typestate. Dropping
/// that typestate cannot silently restore the slot.
#[must_use = "the DTM runtime retains the sole production graph"]
pub struct DtmRuntimeResources {
    config: DtmRuntimeConfig,
    graph_identity: DtmMemoryGraphIdentity,
    idle: Option<DtmSessionIdle>,
}

impl DtmRuntimeResources {
    fn from_claimed_graph(
        default_tx_power_dbm: DtmDefaultTxPowerDbm,
        graph: DtmMemoryGraphCpuOwned,
    ) -> Self {
        let config = DtmRuntimeConfig::new(graph.allocation_config(), default_tx_power_dbm);
        let graph_identity = graph.identity();
        Self {
            config,
            graph_identity,
            idle: Some(DtmSessionIdle::new(graph)),
        }
    }

    /// Bind one real statically placed graph to its composition-owned policy.
    ///
    /// The factory itself supplies the retained allocation configuration to
    /// the lower binding operation. A caller therefore cannot pair a graph
    /// initialized from one allocation policy with a different runtime policy.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut DtmMemoryGraphStorage,
        config: DtmRuntimeConfig,
    ) -> Result<Self, DtmMemoryGraphBindFailure> {
        let graph =
            DtmMemoryGraphStorage::pin_static(storage, config.scheduler_allocation_config())?;
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
        storage: &'static mut DtmMemoryGraphStorage,
        base: DtmMemoryGraphModelAddress,
        config: DtmRuntimeConfig,
    ) -> Result<Self, DtmMemoryGraphBindFailure> {
        let graph = DtmMemoryGraphStorage::pin_static_model(
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
    pub const fn config(&self) -> DtmRuntimeConfig {
        self.config
    }

    /// Physical default transmit-power request retained by this runtime.
    pub const fn default_tx_power_dbm(&self) -> DtmDefaultTxPowerDbm {
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
    ) -> Result<DtmSessionGraphReady, DtmRuntimeSessionBeginError> {
        let idle = self
            .idle
            .take()
            .ok_or(DtmRuntimeSessionBeginError::SessionActive)?;
        Ok(idle.begin_epoch())
    }

    /// Restore one cancelled or fully stopped session to the vacant slot.
    ///
    /// Rejection returns the supplied graph unchanged. It preserves either the
    /// graph already retained by this runtime or its vacant state when the
    /// supplied graph belongs to another pinned storage object.
    pub fn restore_idle(&mut self, idle: DtmSessionIdle) -> Result<(), DtmSessionIdle> {
        if self.idle.is_some() || idle.graph.identity() != self.graph_identity {
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
pub struct DtmSessionIdle {
    graph: DtmMemoryGraphReclaimed,
}

impl DtmSessionIdle {
    /// Capture a newly allocated or otherwise ordinary CPU-owned graph.
    pub fn new(graph: DtmMemoryGraphCpuOwned) -> Self {
        Self {
            graph: graph.into_reclaimed(),
        }
    }

    /// Return a terminal-neutral, fully reclaimed active graph to idle retention.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn from_quiesced(quiesced: DtmQuiescedCpuOwned) -> Self {
        Self {
            graph: quiesced.into_reclaimed_graph(),
        }
    }

    /// Reinitialize the retained graph and begin one fresh allocation epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_epoch(self) -> DtmSessionGraphReady {
        DtmSessionGraphReady {
            graph: self.graph.reinitialize(),
        }
    }
}

/// Fresh CPU-owned graph at the start of one DTM allocation epoch.
#[must_use = "the fresh DTM graph must advance or return to the idle session"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmSessionGraphReady {
    graph: DtmMemoryGraphCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmSessionGraphReady {
    /// Release the fresh CPU owner to the concrete lower event runner.
    ///
    /// This value alone does not claim continuity beyond the edge; the outer
    /// command actor retains each lower typestate until it produces a terminal
    /// Test End owner or an explicit fail-stop owner.
    pub fn into_graph(self) -> DtmMemoryGraphCpuOwned {
        self.graph
    }

    /// Cancel this CPU-only epoch and return its graph to idle retention.
    #[cfg(test)]
    pub fn cancel(self) -> DtmSessionIdle {
        DtmSessionIdle {
            graph: self.graph.into_reclaimed(),
        }
    }
}

/// Ended DTM session retaining its graph and report during response backpressure.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the Test End response must be published before the graph can be reused"]
pub(crate) struct DtmSessionStopping {
    ended: DtmTestEndedCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmSessionStopping {
    /// Retain one lower Test End owner while its response is backpressured.
    ///
    /// The chip stopping runner constructs this bookend only after its exact
    /// active graph reached the CPU-owned completion boundary.
    pub(crate) fn new(ended: DtmTestEndedCpuOwned) -> Self {
        Self { ended }
    }

    /// Borrow the stable role-specific report for response serialization.
    pub(crate) const fn report(&self) -> DtmTestEndReport {
        self.ended.report()
    }

    /// Release the reclaimed graph only after the response was published.
    pub(crate) fn response_published(self) -> DtmSessionIdle {
        DtmSessionIdle {
            graph: self.ended.into_reclaimed_graph(),
        }
    }
}

#[cfg(test)]
mod tests;
