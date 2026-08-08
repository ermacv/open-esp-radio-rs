#![forbid(unsafe_code)]

mod bindings;
mod run;

use core::future::Future;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use open_esp_radio_esp32s31_wifi_embassy::{
    connected_rx_protocol::ConnectedRxProtocolStopped, station::Esp32s31ConnectedTaskGroup,
};
use open_esp_radio_hil_esp32s31_telemetry::task_poll::TaskPollSet;

use crate::{
    console::emergency_log,
    radio_hil::{ConnectedNetworkStackRunner, ConnectedRxProtocol},
};

use super::super::connected_traffic::observe_open_radio_task_polls;

pub(in crate::radio_hil) use bindings::{
    RadioHilConnectedEpochBindings, RadioHilConnectedEpochPolicy, RadioHilConnectedEpochServices,
    RadioHilConnectedEpochStorage,
};
pub(in crate::radio_hil) use run::run_connected_network;

/// HIL executor and cancellation resources shared by one connected epoch.
///
/// Static placement stays in the composition root. This value only exposes
/// the edges needed to run and stop the production network/protocol owners.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedTaskBindings {
    task_polls: &'static TaskPollSet,
    task_poll_telemetry: bool,
    protocol_stop: &'static Signal<CriticalSectionRawMutex, ()>,
    protocol_stopped: &'static Signal<CriticalSectionRawMutex, ConnectedRxProtocolStopped<'static>>,
    traffic_stop: &'static Signal<CriticalSectionRawMutex, ()>,
    traffic_stopped: &'static Signal<CriticalSectionRawMutex, ()>,
}

impl RadioHilConnectedTaskBindings {
    pub(in crate::radio_hil) const fn new(
        task_polls: &'static TaskPollSet,
        task_poll_telemetry: bool,
        protocol_stop: &'static Signal<CriticalSectionRawMutex, ()>,
        protocol_stopped: &'static Signal<
            CriticalSectionRawMutex,
            ConnectedRxProtocolStopped<'static>,
        >,
        traffic_stop: &'static Signal<CriticalSectionRawMutex, ()>,
        traffic_stopped: &'static Signal<CriticalSectionRawMutex, ()>,
    ) -> Self {
        Self {
            task_polls,
            task_poll_telemetry,
            protocol_stop,
            protocol_stopped,
            traffic_stop,
            traffic_stopped,
        }
    }

    pub(in crate::radio_hil) const fn radio_polls(
        self,
    ) -> &'static open_esp_radio_hil_esp32s31_telemetry::task_poll::TaskPollCounters {
        self.task_polls.radio()
    }

    pub(in crate::radio_hil) const fn telemetry_enabled(self) -> bool {
        self.task_poll_telemetry
    }
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn connected_network_stack_task(
    mut runner: ConnectedNetworkStackRunner,
    bindings: RadioHilConnectedTaskBindings,
) {
    observe_open_radio_task_polls(
        runner.run(),
        bindings.task_polls.network(),
        bindings.task_poll_telemetry,
    )
    .await
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn connected_rx_protocol_task(
    protocol: ConnectedRxProtocol,
    bindings: RadioHilConnectedTaskBindings,
) {
    let stopped = observe_open_radio_task_polls(
        protocol.run_until_stopped(bindings.protocol_stop.wait()),
        bindings.task_polls.protocol(),
        bindings.task_poll_telemetry,
    )
    .await;
    let shutdown = stopped.shutdown();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stop \
         queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    ));
    bindings.protocol_stopped.signal(stopped);
}

/// Adapter between the production finite-stop contract and HIL task signals.
pub(in crate::radio_hil) struct RadioHilConnectedTaskGroup {
    bindings: RadioHilConnectedTaskBindings,
}

impl RadioHilConnectedTaskGroup {
    pub(in crate::radio_hil) const fn new(bindings: RadioHilConnectedTaskBindings) -> Self {
        Self { bindings }
    }
}

impl Esp32s31ConnectedTaskGroup for RadioHilConnectedTaskGroup {
    type Stopped = ConnectedRxProtocolStopped<'static>;

    fn request_stop(&mut self) {
        self.bindings.traffic_stop.signal(());
        self.bindings.protocol_stop.signal(());
    }

    fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
        async {
            self.bindings.traffic_stopped.wait().await;
            self.bindings.protocol_stopped.wait().await
        }
    }
}
