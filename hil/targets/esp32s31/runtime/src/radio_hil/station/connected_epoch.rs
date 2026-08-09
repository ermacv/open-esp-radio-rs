#![forbid(unsafe_code)]

mod bindings;
mod run;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio_hil_esp32s31_telemetry::task_poll::TaskPollSet;
use open_esp_radio_wifi_embassy::connected_tasks::{
    ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskController,
    ConnectedTaskEndpoint, ConnectedTaskGroupWithAuxiliary,
};

use crate::{
    console::emergency_log,
    radio_hil::{
        ConnectedNetworkStackRunner, ConnectedRxProtocol, RadioHilConnectedRxProtocolStopped,
    },
};

use super::super::connected_traffic::observe_open_radio_task_polls;

pub(in crate::radio_hil) use bindings::{
    RadioHilConnectedEpochBindings, RadioHilConnectedEpochPolicy, RadioHilConnectedEpochServices,
    RadioHilConnectedEpochStorage, RadioHilConnectedStaticResourceError,
    RadioHilInitialConnectedStaticResources,
};
pub(in crate::radio_hil) use run::run_connected_network;

pub(in crate::radio_hil) type RadioHilConnectedTrafficTaskEndpoint =
    ConnectedTaskEndpoint<'static, CriticalSectionRawMutex, ()>;

/// HIL executor and cancellation resources shared by one connected epoch.
///
/// Static placement stays in the composition root. This value only exposes
/// the edges needed to run and stop the production network/protocol owners.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedTaskBindings {
    task_polls: &'static TaskPollSet,
    task_poll_telemetry: bool,
    protocol: &'static ConnectedTaskControlResources<
        CriticalSectionRawMutex,
        RadioHilConnectedRxProtocolStopped,
    >,
    traffic: &'static ConnectedTaskControlResources<CriticalSectionRawMutex, ()>,
}

impl RadioHilConnectedTaskBindings {
    pub(in crate::radio_hil) const fn new(
        task_polls: &'static TaskPollSet,
        task_poll_telemetry: bool,
        protocol: &'static ConnectedTaskControlResources<
            CriticalSectionRawMutex,
            RadioHilConnectedRxProtocolStopped,
        >,
        traffic: &'static ConnectedTaskControlResources<CriticalSectionRawMutex, ()>,
    ) -> Self {
        Self {
            task_polls,
            task_poll_telemetry,
            protocol,
            traffic,
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

    pub(in crate::radio_hil) fn start_epoch(
        self,
    ) -> Result<
        (
            RadioHilConnectedTaskGroup,
            ConnectedTaskEndpoint<
                'static,
                CriticalSectionRawMutex,
                RadioHilConnectedRxProtocolStopped,
            >,
            RadioHilConnectedTrafficTaskEndpoint,
        ),
        ConnectedTaskControlError,
    > {
        let (protocol, protocol_endpoint) = self.protocol.split()?;
        let (traffic, traffic_endpoint) = self.traffic.split()?;
        Ok((
            ConnectedTaskGroupWithAuxiliary::new(protocol, traffic),
            protocol_endpoint,
            traffic_endpoint,
        ))
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
    endpoint: ConnectedTaskEndpoint<
        'static,
        CriticalSectionRawMutex,
        RadioHilConnectedRxProtocolStopped,
    >,
) {
    observe_open_radio_task_polls(
        protocol.run_controlled_task(endpoint, |shutdown| {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stop \
                 queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
                shutdown.queued_frames,
                shutdown.retained_frames,
                shutdown.reorder_commands,
                shutdown.active_reorders,
            ));
        }),
        bindings.task_polls.protocol(),
        bindings.task_poll_telemetry,
    )
    .await;
}

/// Both HIL tasks must consume their current epoch endpoints before the
/// protocol owner can be reused. A dropped endpoint poisons its static control
/// resource instead of allowing a later epoch to clear a raw signal.
pub(in crate::radio_hil) type RadioHilConnectedTaskGroup = ConnectedTaskGroupWithAuxiliary<
    ConnectedTaskController<'static, CriticalSectionRawMutex, RadioHilConnectedRxProtocolStopped>,
    ConnectedTaskController<'static, CriticalSectionRawMutex, ()>,
>;
