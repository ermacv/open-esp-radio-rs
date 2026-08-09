#![forbid(unsafe_code)]

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::{
    RadioConfig, WifiConfig, WifiMonitorConfig,
    esp32s31::{
        RADIO_CAPABILITIES,
        wifi::{device::runtime::Esp32s31WifiStopped, mac::rx::RxPhyInfo},
    },
    wifi::{
        softmac::{MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink},
        sta::station::StaReconnectPolicy,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    embassy_irq::Esp32s31MacInterruptEpoch,
    monitor::{
        Esp32s31MonitorCompletion, Esp32s31MonitorControlResources, Esp32s31MonitorInterrupts,
        Esp32s31MonitorMemory, Esp32s31MonitorTaskResources, prepare_esp32s31_monitor_task,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
    station::{
        Esp32s31StationConfig, Esp32s31StationExit, Esp32s31StationStartResources,
        materialize_esp32s31_station, prepare_esp32s31_station_task,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral, mac_interrupt_epoch::EspHalMacInterruptRoute,
};
use static_cell::ConstStaticCell;

use crate::{
    console::emergency_log,
    radio_hil::{
        OPEN_RADIO_IRQ_RUNTIME, OPEN_RADIO_POWER_IRQ_RUNTIME,
        OPEN_RADIO_ROLE_TRANSITION_MONITOR_RX, station_epoch_coordinator,
    },
};

use super::{
    RadioHilStationEngine, RadioHilStationEngineObserver, RadioHilStationEnginePort,
    RadioHilStationReusableResources, radio_hil_station_discovery, station_restart_control_task,
    try_reclaim_station_runtime, try_restart_station_runtime,
};

// This arena proves only the stopped-owner transition; it is not the monitor
// capture qualification pool. The controller requests stop before running the
// task, so retaining a jumbo four-buffer capture ring here needlessly removes
// SRAM from the production station executor stack. Two station-sized buffers
// are sufficient to exercise real DMA/IRQ start and quiescence without
// silently changing the memory profile of every STA throughput image.
const TRANSITION_MONITOR_DESCRIPTOR_COUNT: usize = 2;
const TRANSITION_MONITOR_BUFFER_SIZE: usize = 1_700;
const TRANSITION_MONITOR_STORAGE_SIZE: usize = TRANSITION_MONITOR_BUFFER_SIZE + 4;

pub(in crate::radio_hil) type TransitionMonitorRxStorage = Esp32s31RxDmaStorage<
    TRANSITION_MONITOR_DESCRIPTOR_COUNT,
    TRANSITION_MONITOR_BUFFER_SIZE,
    TRANSITION_MONITOR_STORAGE_SIZE,
>;

// Descriptor addresses are permanent monitor-DMA metadata. Const static
// initialization keeps even the transition-only arena out of the station
// task stack and makes its one-time ownership explicit.
static TRANSITION_MONITOR_ADDRESSES: ConstStaticCell<[u32; TRANSITION_MONITOR_DESCRIPTOR_COUNT]> =
    ConstStaticCell::new([0; TRANSITION_MONITOR_DESCRIPTOR_COUNT]);
static TRANSITION_MONITOR_CONTROL: ConstStaticCell<
    Esp32s31MonitorControlResources<CriticalSectionRawMutex>,
> = ConstStaticCell::new(Esp32s31MonitorControlResources::new());

#[derive(Default)]
struct TransitionMonitorSink;

impl MonitorSink<RxPhyInfo> for TransitionMonitorSink {
    fn try_publish(&mut self, _frame: MonitorFrame<'_, RxPhyInfo>) -> MonitorPublishOutcome {
        // This ownership qualification has no capture consumer. Report the
        // deliberate discard honestly while keeping the sink non-blocking.
        MonitorPublishOutcome::Dropped(MonitorDropReason::Filtered)
    }
}

/// Prove a finite station -> monitor -> station owner round trip without
/// reacquiring PAC or manufacturing an interrupt setup token.
///
/// The returned monitor owner starts a second real station task. For the
/// normal connected-stop frontier that task performs running scan, join,
/// security and connected entry before its controller requests a clean stop.
pub(in crate::radio_hil) async fn qualify_station_monitor_station_owner_round_trip<'security>(
    spawner: Spawner,
    target_ssid: &[u8],
    wifi: Esp32s31WifiStopped<EspHalRadioPeripheral>,
    station_resources: RadioHilStationReusableResources<'security>,
    route: EspHalMacInterruptRoute,
) -> bool {
    let discovery = match radio_hil_station_discovery(target_ssid) {
        Ok(discovery) => discovery,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-restart-discovery error={error}"
            ));
            return false;
        }
    };
    let plan = RadioConfig::wifi(WifiConfig::monitor(WifiMonitorConfig::normalized()))
        .validate(RADIO_CAPABILITIES)
        .expect("ESP32-S31 capabilities accept normalized standalone monitor")
        .standalone_wifi_monitor()
        .expect("monitor-only topology materializes a standalone monitor plan");
    let storage = OPEN_RADIO_ROLE_TRANSITION_MONITOR_RX.take();
    let addresses = TRANSITION_MONITOR_ADDRESSES.take();
    let memory = match Esp32s31MonitorMemory::new(storage, addresses) {
        Ok(memory) => memory,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-memory error={error:?}"
            ));
            return false;
        }
    };
    let interrupts = Esp32s31MonitorInterrupts::new(
        route,
        &OPEN_RADIO_IRQ_RUNTIME,
        &OPEN_RADIO_POWER_IRQ_RUNTIME,
    );
    let control = TRANSITION_MONITOR_CONTROL.take();
    let resources =
        Esp32s31MonitorTaskResources::new(memory, TransitionMonitorSink, interrupts, control);
    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(plan, wifi, resources) {
        Ok(task) => task,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-materialize error={:?}",
                failure.error,
            ));
            return false;
        }
    };
    controller.request_stop();
    let first_report = match task.run().await {
        Ok(report) => report,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-run error={:?}",
                failure.error,
            ));
            return false;
        }
    };
    let stopped = match task.try_into_stopped() {
        Ok(stopped) => stopped,
        Err(_) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-regroup error=active-owner"
            ));
            return false;
        }
    };
    let completion = controller.wait_completion().await;
    if completion != Esp32s31MonitorCompletion::Stopped {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-stopped-role-monitor-regroup completion={completion:?}"
        ));
        return false;
    }
    drop(controller);

    // Rebind only the exact owner graph returned by the first monitor. This
    // proves that a clean role epoch is reusable without reinitializing its
    // DMA arena, control mailbox, interrupt route or common Wi-Fi owner.
    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(
        stopped.plan,
        stopped.wifi,
        stopped.resources.into_task_resources(),
    ) {
        Ok(task) => task,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=production-stopped-role-monitor-restart-materialize error={:?}",
                failure.error,
            ));
            return false;
        }
    };
    controller.request_stop();
    let report = match task.run().await {
        Ok(report) => report,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-restart-run error={:?}",
                failure.error,
            ));
            return false;
        }
    };
    let stopped = match task.try_into_stopped() {
        Ok(stopped) => stopped,
        Err(_) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-stopped-role-monitor-restart-regroup error=active-owner"
            ));
            return false;
        }
    };
    let completion = controller.wait_completion().await;
    if completion != Esp32s31MonitorCompletion::Stopped {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-stopped-role-monitor-restart-regroup completion={completion:?}"
        ));
        return false;
    }
    drop(controller);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-stopped-role-monitor-restart \
         first_completed={} first_published={} second_completed={} second_published={}",
        first_report.receive.completed_descriptors,
        first_report.receive.published_frames,
        report.receive.completed_descriptors,
        report.receive.published_frames,
    ));
    let monitor_channel = stopped.wifi.current_channel().primary();
    let monitor_resources = stopped.resources.into_parts();
    let monitor_interrupts = monitor_resources.interrupts.into_parts();
    let station = materialize_esp32s31_station(stopped.wifi, station_resources);
    let interrupt_epoch = Esp32s31MacInterruptEpoch::new(
        monitor_interrupts.route,
        station.interrupt_setup,
        monitor_interrupts.mac_runtime,
        monitor_interrupts.power_runtime,
    );
    let (owner, station_control) = match try_restart_station_runtime(
        station.owner,
        interrupt_epoch,
        station.registers,
        station.resources,
    ) {
        Ok(restarted) => restarted,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-restart-materialize error={:?} resources={:?}",
                failure.error,
                failure.resources.stopped_phase(),
            ));
            let _returned = failure.registers;
            return false;
        }
    };
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
        .expect("fixed HIL station restart policy is valid");
    let (controller, station_task) = match prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy),
        Esp32s31StationStartResources::new(owner),
        station_control,
        RadioHilStationEngine::with_observer(
            RadioHilStationEnginePort::new(),
            discovery,
            RadioHilStationEngineObserver,
        ),
    ) {
        Ok(task) => task,
        Err(failure) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-restart-control error={:?}",
                failure.error,
            ));
            let _retained_station_owner = failure;
            return false;
        }
    };
    spawner.spawn(
        station_restart_control_task(controller, station_epoch_coordinator())
            .unwrap_or_else(|_| panic!("station restart controller task allocation failed")),
    );
    let returned = match station_task.run().await {
        Esp32s31StationExit::Stopped {
            resources,
            progress,
            reason,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-station-restart-stop \
                 connected_epochs={} attempts={} reason={reason:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            resources
        }
        Esp32s31StationExit::RetryExhausted {
            progress, failure, ..
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-station-restart-exhausted \
                 connected_epochs={} attempts={} failure={failure:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            return false;
        }
        Esp32s31StationExit::Terminal {
            progress, failure, ..
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-station-restart-terminal \
                 connected_epochs={} attempts={} failure={failure:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            return false;
        }
    };
    // `connected_epochs` counts connected attempts that subsequently returned
    // through the disconnected path. An explicit controller stop from the
    // active connected epoch intentionally leaves that counter at zero, so
    // use the coordinator's independently observed connected-entry edge.
    if !station_epoch_coordinator().internal_epoch_reached_connected() {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-station-restart-stop error=no-connected-entry"
        ));
        return false;
    }
    let (owner, _runner) = returned.into_parts();
    let reclaimed = match try_reclaim_station_runtime(owner) {
        Ok(reclaimed) => reclaimed,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-restart-reclaim error={error:?}"
            ));
            return false;
        }
    };
    let (returned_route, interrupt_setup, mac_runtime, power_runtime) =
        match reclaimed.interrupt.try_into_inactive_parts() {
            Ok(parts) => parts,
            Err(_) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=production-station-restart-regroup error=interrupt-active"
                ));
                return false;
            }
        };
    let channel = reclaimed
        .channel
        .expect("a completed restarted station epoch has a selected channel");
    let mut role = reclaimed.role;
    role.set_current_channel(channel);
    let station = role.into_stopped(reclaimed.registers, interrupt_setup, reclaimed.resources);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-stopped-role-round-trip monitor_channel={} station_channel={} \
         service_wakes={} completed={} published={} dropped={} station_resources={:?}",
        monitor_channel,
        station.wifi.current_channel().primary(),
        report.rx_service_wakes,
        report.receive.completed_descriptors,
        report.receive.published_frames,
        report.receive.dropped_frames,
        station.resources.stopped_phase(),
    ));
    let _returned = (
        station,
        stopped.plan,
        monitor_resources.memory,
        monitor_resources.sink,
        monitor_resources.control,
        returned_route,
        mac_runtime,
        power_runtime,
    );
    true
}
