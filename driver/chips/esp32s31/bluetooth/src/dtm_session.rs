//! Executor-neutral bookends for one reusable DTM memory-graph session.
//!
//! This layer deliberately does not model controller execution. It provides
//! only the honest bookends around that missing session pump: an idle graph can
//! begin a fresh CPU-owned epoch, and an already ended test retains its graph
//! and report until response publication. The allocation configuration is
//! inseparable from the graph binding throughout both bookends.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphReclaimed,
};

#[cfg(any(target_arch = "riscv32", test))]
use crate::dtm_event_prepare::{BluetoothDtmTestEndReport, BluetoothDtmTestEndedCpuOwned};

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
    pub fn begin_epoch(self) -> BluetoothDtmSessionGraphReady {
        BluetoothDtmSessionGraphReady {
            graph: self.graph.reinitialize(),
        }
    }
}

/// Fresh CPU-owned graph at the start of one DTM allocation epoch.
#[must_use = "the fresh DTM graph must advance or return to the idle session"]
pub struct BluetoothDtmSessionGraphReady {
    graph: BluetoothDtmMemoryGraphCpuOwned,
}

impl BluetoothDtmSessionGraphReady {
    /// Release the fresh CPU owner to the concrete lower session pump.
    ///
    /// No session continuity is claimed beyond this edge. The future pump must
    /// retain the exact lower typestate until it can produce a Test End owner.
    pub fn into_graph(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.graph
    }

    /// Cancel this CPU-only epoch and return its graph to idle retention.
    pub fn cancel(self) -> BluetoothDtmSessionIdle {
        BluetoothDtmSessionIdle {
            graph: self.graph.into_reclaimed(),
        }
    }
}

/// Ended DTM session retaining its graph and report during response backpressure.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the Test End response must be published before the graph can be reused"]
pub struct BluetoothDtmSessionStopping {
    ended: BluetoothDtmTestEndedCpuOwned,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmSessionStopping {
    /// Retain one lower Test End owner while its response is backpressured.
    ///
    /// Constructing this bookend does not prove that it belongs to an earlier
    /// [`BluetoothDtmSessionGraphReady`]. That continuity remains the job of a
    /// future concrete session pump.
    pub fn new(ended: BluetoothDtmTestEndedCpuOwned) -> Self {
        Self { ended }
    }

    /// Borrow the stable role-specific report for response serialization.
    pub const fn report(&self) -> BluetoothDtmTestEndReport {
        self.ended.report()
    }

    /// Release the reclaimed graph only after the response was published.
    pub fn response_published(self) -> BluetoothDtmSessionIdle {
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

    use super::BluetoothDtmSessionIdle;

    fn graph() -> BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_6800)
            .expect("model address uses the controller SRAM syntax");
        let config = BluetoothDtmSchedulerAllocationConfig::new(1, 2, 5, 1);
        BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config)
            .expect("model graph fits in controller SRAM")
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
}
