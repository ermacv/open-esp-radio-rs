//! HIL image capabilities, independent of whether the product task is linked.

use open_esp_radio_hil_protocol::{Capabilities, FeatureCapabilities, MAX_WIRE_FRAME_BYTES};

pub(crate) const OPEN_RADIO_TASK_POLL_TELEMETRY: bool =
    cfg!(feature = "connected-datapath-poll-telemetry");
pub(crate) const OPEN_RADIO_MAC_IRQ_TELEMETRY: bool = cfg!(feature = "mac-irq-telemetry");
pub(crate) const OPEN_RADIO_RX_DELIVERY_TELEMETRY: bool = cfg!(feature = "rx-delivery-telemetry");
pub(crate) const OPEN_RADIO_DRIVER_OBSERVATION: bool = cfg!(feature = "driver-observation");
pub(crate) const OPEN_RADIO_TCP_CHUNK_CAPACITY: usize = 32_768;

// The command's per-frame payload policy, independent of radio socket storage.
const MEMORY_BENCHMARK_PAYLOAD_CAPACITY: u16 = 4096;

pub const fn hil_capabilities() -> Capabilities {
    Capabilities {
        features: FeatureCapabilities {
            udp: !cfg!(feature = "memory-benchmark"),
            tcp: !cfg!(feature = "memory-benchmark"),
            rx: !cfg!(feature = "memory-benchmark"),
            tx: !cfg!(feature = "memory-benchmark"),
            bidirectional: !cfg!(feature = "memory-benchmark"),
            runtime_initialization: !cfg!(feature = "memory-benchmark"),
            runtime_configuration: !cfg!(feature = "memory-benchmark"),
            structured_evidence: true,
            udp_multi_flow: !cfg!(feature = "memory-benchmark"),
            startup_artifact: !cfg!(feature = "memory-benchmark"),
            station_epoch_control: !cfg!(feature = "memory-benchmark"),
            wifi_role_control: !cfg!(feature = "memory-benchmark"),
            wifi_access_point: !cfg!(feature = "memory-benchmark"),
            simultaneous_station_access_point: !cfg!(feature = "memory-benchmark"),
            wifi_monitor_capture: !cfg!(feature = "memory-benchmark"),
            station_lifecycle_events: !cfg!(feature = "memory-benchmark"),
            driver_observation_evidence: OPEN_RADIO_DRIVER_OBSERVATION,
            rx_delivery_evidence: OPEN_RADIO_RX_DELIVERY_TELEMETRY,
            task_poll_evidence: OPEN_RADIO_TASK_POLL_TELEMETRY,
            tx_architecture_probe: cfg!(feature = "tx-architecture-probes"),
            core0_rx_cycle_evidence: cfg!(any(
                feature = "core0-rx-cycle-telemetry",
                feature = "core0-rx-coarse-telemetry"
            )),
            mac_irq_evidence: OPEN_RADIO_MAC_IRQ_TELEMETRY,
            psram_task_stack: cfg!(feature = "psram-task-stack"),
            network_scheduler_evidence: false,
            data_plane_placement: !cfg!(feature = "memory-benchmark"),
            timebase_probe: true,
            memory_benchmark: cfg!(feature = "memory-benchmark"),
            ieee802154_event_status_probe: cfg!(feature = "ieee802154-event-status-probe"),
            ieee802154_ed_event_probe: cfg!(feature = "ieee802154-ed-event-probe"),
        },
        maximum_payload_bytes: if cfg!(feature = "memory-benchmark") {
            MEMORY_BENCHMARK_PAYLOAD_CAPACITY
        } else {
            OPEN_RADIO_TCP_CHUNK_CAPACITY as u16
        },
        maximum_wire_frame_bytes: MAX_WIRE_FRAME_BYTES as u16,
    }
}
