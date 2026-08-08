//! One-shot materialization of a standalone ESP32-S31 monitor owner graph.

#![forbid(unsafe_code)]

use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_hal::RadioRegisters;
use open_esp_radio_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError,
    phy_cold::{PhyCalibrationRecord, PhyColdState},
};
use open_esp_radio_esp32s31_registers::MacInterruptSetup;
use open_esp_radio_esp32s31_wifi::{
    mac_start::Esp32s31WifiMacStartReport,
    runtime::{Esp32s31WifiRuntimeParts, Esp32s31WifiRuntimeTransitionReport, Esp32s31WifiStopped},
    switch_esp32s31_wifi_channel,
};
use open_esp_radio_esp32s31_wifi_dma::rx_storage::RxDmaStorageError;
use open_esp_radio_esp32s31_wifi_mac::{
    init::activate_promiscuous_receive, irq::MacInterruptRoute, rx::RxPhyInfo,
};
use open_esp_radio_ieee80211::channel::WifiChannel;
use open_esp_radio_wifi_softmac::{MonitorSink, WifiStandaloneMonitorPlan};

use crate::{
    embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch},
    monitor::{
        Esp32s31MonitorCommandReceiver, Esp32s31MonitorCompletion, Esp32s31MonitorControlError,
        Esp32s31MonitorControlResources, Esp32s31MonitorController,
    },
    monitor_rx::{Esp32s31MonitorPrepareError, Esp32s31MonitorRx},
    monitor_service::{
        Esp32s31MonitorRunFailure, Esp32s31MonitorRunReport, Esp32s31MonitorService,
        Esp32s31MonitorStoppedAccessError,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
};

/// Permanently located DMA arena and its once-derived address table.
///
/// Construction happens before any radio owner moves. Once successful the
/// address table becomes immutable for the rest of the firmware lifetime,
/// matching the DMA ring's real safety requirement.
#[derive(Clone, Copy)]
pub struct Esp32s31MonitorMemory<
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    buffer_addresses: &'static [u32; COUNT],
    descriptor_base: u32,
}

impl<const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub fn new(
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        buffer_addresses: &'static mut [u32; COUNT],
    ) -> Result<Self, RxDmaStorageError> {
        let descriptor_base = storage.dma_layout(buffer_addresses)?;
        Ok(Self {
            storage,
            buffer_addresses,
            descriptor_base,
        })
    }
}

/// Platform interrupt binding for one monitor task epoch.
pub struct Esp32s31MonitorInterrupts<'runtime, R, M: RawMutex>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
{
    route: R,
    mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
}

impl<'runtime, R, M: RawMutex> Esp32s31MonitorInterrupts<'runtime, R, M>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
{
    pub fn new(
        route: R,
        mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
        power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    ) -> Self {
        Self {
            route,
            mac_runtime,
            power_runtime,
        }
    }
}

/// Runtime-owned resources consumed together when a monitor role starts.
///
/// The frame arena and address table are board placement policy. The route,
/// wake runtimes and sink are executor integration. Grouping them prevents a
/// caller from accidentally pairing an RX ring with a different IRQ epoch.
struct Esp32s31MonitorResources<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    dma: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    sink: S,
    route: R,
    mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
}

impl<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    const fn new(
        dma: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        sink: S,
        route: R,
        mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
        power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    ) -> Self {
        Self {
            dma,
            sink,
            route,
            mac_runtime,
            power_runtime,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorBuildError {
    Control(Esp32s31MonitorControlError),
    Receive(Esp32s31MonitorPrepareError),
}

/// Failed materialization retaining both the MAC owner and every runtime
/// resource for retry or explicit reset policy.
struct Esp32s31MonitorBuildFailure<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: Esp32s31MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub wifi: Esp32s31WifiStopped<P>,
    pub resources:
        Esp32s31MonitorResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31MonitorBuildReport {
    pub start: Esp32s31WifiMacStartReport,
    pub cold_interrupt_mask: u32,
    pub descriptor_base: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorChannelSwitchError {
    Active(Esp32s31MonitorStoppedAccessError),
    Phy(PhyTargetPortError),
}

/// Complete stopped standalone-monitor owner.
///
/// Calling `run_until_stopped` on the contained service creates a finite live
/// IRQ/DMA epoch. The PHY state remains here for a later stopped-only channel
/// switch; it is never duplicated in the capture consumer.
struct Esp32s31MonitorOwner<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    service: Esp32s31MonitorService<
        'static,
        'runtime,
        RadioRegisters,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    phy: PhyColdState,
    calibration_record: Option<PhyCalibrationRecord>,
    report: Esp32s31MonitorBuildReport,
    current_channel: WifiChannel,
    plan: WifiStandaloneMonitorPlan,
    memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorOwner<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn report(&self) -> Esp32s31MonitorBuildReport {
        self.report
    }

    pub const fn calibration_record(&self) -> Option<&PhyCalibrationRecord> {
        self.calibration_record.as_ref()
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    /// Return the common Wi-Fi owner and every reusable monitor resource only
    /// after IRQ routing and RX DMA are both proven inactive.
    fn try_into_stopped(
        self,
    ) -> Result<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
    > {
        let Self {
            service,
            phy,
            calibration_record,
            report,
            current_channel,
            plan,
            memory,
        } = self;
        let (registers, receive, sink, interrupts, platform) = match service.try_into_parts() {
            Ok(parts) => parts,
            Err(service) => {
                return Err(Self {
                    service,
                    phy,
                    calibration_record,
                    report,
                    current_channel,
                    plan,
                    memory,
                });
            }
        };
        let (route, interrupt_setup, mac_runtime, power_runtime) =
            match interrupts.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(_) => unreachable!(
                    "a decomposable monitor service contains an inactive interrupt epoch"
                ),
            };
        let _halted = receive
            .into_halted()
            .unwrap_or_else(|_| unreachable!("a decomposable monitor service has stopped RX"));
        let wifi = Esp32s31WifiStopped::from_runtime_parts(Esp32s31WifiRuntimeParts {
            platform,
            registers,
            interrupt_setup,
            phy,
            calibration_record,
            start_report: report.start,
            transition_report: Esp32s31WifiRuntimeTransitionReport {
                cold_interrupt_mask: report.cold_interrupt_mask,
            },
            current_channel,
        });
        Ok(Esp32s31MonitorStopped {
            wifi,
            plan,
            resources: Esp32s31MonitorStoppedResources {
                memory,
                sink,
                interrupts: Esp32s31MonitorInterrupts::new(route, mac_runtime, power_runtime),
            },
        })
    }

    /// Run inside the long-lived role task until its application handle asks
    /// for shutdown. Completion is published only after IRQ and DMA are
    /// confirmed quiescent, or as `Faulted` while this task still retains every
    /// hardware-visible owner for reset.
    pub async fn run_controlled(
        &mut self,
        control: &mut Esp32s31MonitorCommandReceiver<'_, M>,
    ) -> Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<R::Error>> {
        let result = self.service.run_until_stopped(control.wait_stop()).await;
        control.complete(if self.service.is_quiescent() {
            Esp32s31MonitorCompletion::Stopped
        } else {
            Esp32s31MonitorCompletion::Faulted
        });
        result
    }

    /// Retune only while the monitor's IRQ route and RX walker are stopped.
    pub async fn switch_channel<D, O>(
        &mut self,
        channel: WifiChannel,
        observer: &mut O,
    ) -> Result<(), Esp32s31MonitorChannelSwitchError>
    where
        D: PhyAsyncDelay,
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
            + open_esp_radio_esp32s31_hal::phy_temperature::PhyTemperatureSystemControl
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
        O: PhyTargetObserver,
    {
        let (registers, platform) = self
            .service
            .stopped_radio_mut()
            .map_err(Esp32s31MonitorChannelSwitchError::Active)?;
        switch_esp32s31_wifi_channel::<D, _, _>(
            &mut self.phy,
            channel,
            platform,
            registers,
            observer,
        )
        .await
        .map_err(Esp32s31MonitorChannelSwitchError::Phy)?;
        self.current_channel = channel;
        Ok(())
    }
}

/// Join a checked standalone-monitor plan, common-MAC owner and runtime
/// resources without exposing intermediate DMA or interrupt setup tokens.
fn prepare_esp32s31_monitor<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    plan: WifiStandaloneMonitorPlan,
    mut wifi: Esp32s31WifiStopped<P>,
    resources: Esp32s31MonitorResources<
        'runtime,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
) -> Result<
    Esp32s31MonitorOwner<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    Esp32s31MonitorBuildFailure<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
>
where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    let cold_interrupt_mask = wifi.transition_report().cold_interrupt_mask;
    let receive = {
        let (registers, _) = wifi.radio_mut();
        activate_promiscuous_receive(registers);
        Esp32s31MonitorRx::prepare_initial(
            plan,
            registers,
            resources.dma.storage,
            resources.dma.descriptor_base,
            resources.dma.buffer_addresses,
        )
    };
    let receive = match receive {
        Ok(receive) => receive,
        Err(error) => {
            return Err(Esp32s31MonitorBuildFailure {
                error: Esp32s31MonitorBuildError::Receive(error),
                plan,
                wifi,
                resources,
            });
        }
    };
    let start = wifi.start_report();
    let runtime = wifi.into_runtime_parts();
    let current_channel = runtime.current_channel;
    let interrupts = Esp32s31MacInterruptEpoch::new(
        resources.route,
        runtime.interrupt_setup,
        resources.mac_runtime,
        resources.power_runtime,
    );
    let service = Esp32s31MonitorService::new(
        runtime.registers,
        receive,
        resources.sink,
        interrupts,
        runtime.platform,
    );
    Ok(Esp32s31MonitorOwner {
        service,
        phy: runtime.phy,
        calibration_record: runtime.calibration_record,
        report: Esp32s31MonitorBuildReport {
            start,
            cold_interrupt_mask,
            descriptor_base: resources.dma.descriptor_base,
        },
        current_channel,
        plan,
        memory: resources.dma,
    })
}

/// Board-owned resources for one standalone monitor task.
///
/// This is the only public monitor materialization input. DMA placement,
/// capture publication, interrupt routing, executor wakes and control storage
/// move together, so application code cannot pair pieces from different radio
/// epochs.
pub struct Esp32s31MonitorTaskResources<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    runtime: Esp32s31MonitorResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    control: &'runtime mut Esp32s31MonitorControlResources<M>,
}

impl<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorTaskResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub fn new(
        memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        sink: S,
        interrupts: Esp32s31MonitorInterrupts<'runtime, R, M>,
        control: &'runtime mut Esp32s31MonitorControlResources<M>,
    ) -> Self {
        Self {
            runtime: Esp32s31MonitorResources::new(
                memory,
                sink,
                interrupts.route,
                interrupts.mac_runtime,
                interrupts.power_runtime,
            ),
            control,
        }
    }
}

/// Reusable board/executor resources returned by a stopped monitor role.
///
/// The common interrupt setup token is deliberately absent: it belongs to
/// [`Esp32s31WifiStopped`]. Pairing these resources with another common Wi-Fi
/// owner therefore starts a fresh role epoch through the normal builder.
pub struct Esp32s31MonitorStoppedResources<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    sink: S,
    interrupts: Esp32s31MonitorInterrupts<'runtime, R, M>,
}

impl<
    'runtime,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorStoppedResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    /// Bind a fresh control mailbox before materializing another monitor task.
    pub fn with_control(
        self,
        control: &'runtime mut Esp32s31MonitorControlResources<M>,
    ) -> Esp32s31MonitorTaskResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
    {
        Esp32s31MonitorTaskResources::new(self.memory, self.sink, self.interrupts, control)
    }
}

/// Fully dematerialized standalone monitor role.
///
/// This value can exist only after the task proved that no ISR route or DMA
/// walker remains active. `wifi` may now be moved into another Wi-Fi role;
/// `resources` remain role-local board/executor policy.
pub struct Esp32s31MonitorStopped<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub wifi: Esp32s31WifiStopped<P>,
    pub plan: WifiStandaloneMonitorPlan,
    pub resources: Esp32s31MonitorStoppedResources<
        'runtime,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
}

/// Failed task materialization with the complete retryable owner frontier.
pub struct Esp32s31MonitorTaskBuildFailure<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: Esp32s31MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub wifi: Esp32s31WifiStopped<P>,
    pub resources:
        Esp32s31MonitorTaskResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

/// Executor-side owner of all hardware used by a standalone monitor role.
///
/// Applications retain only [`Esp32s31MonitorController`]. This value belongs
/// in one long-lived executor task and never crosses the application control
/// plane while its IRQ/DMA epoch is active.
pub struct Esp32s31MonitorTask<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    owner: Esp32s31MonitorOwner<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    control: Esp32s31MonitorCommandReceiver<'runtime, M>,
}

impl<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn report(&self) -> Esp32s31MonitorBuildReport {
        self.owner.report()
    }

    pub const fn calibration_record(&self) -> Option<&PhyCalibrationRecord> {
        self.owner.calibration_record()
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.owner.current_channel()
    }

    /// Run the finite role epoch. The task-side command receiver is private,
    /// so only the paired controller can request the terminal edge.
    pub async fn run(
        &mut self,
    ) -> Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<R::Error>> {
        self.owner.run_controlled(&mut self.control).await
    }

    /// Consume a task only after its hardware epoch is fully stopped.
    ///
    /// Success acknowledges `Stopped` to the paired controller and returns
    /// both the common Wi-Fi owner and role-local reusable resources. If IRQ
    /// routing or RX DMA is still active, the complete task is returned.
    pub fn try_into_stopped(
        self,
    ) -> Result<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
    > {
        let Self { owner, mut control } = self;
        match owner.try_into_stopped() {
            Ok(stopped) => {
                control.complete(Esp32s31MonitorCompletion::Stopped);
                drop(control);
                Ok(stopped)
            }
            Err(owner) => Err(Self { owner, control }),
        }
    }

    /// Retune only while the contained IRQ/DMA service is stopped.
    pub async fn switch_channel<D, O>(
        &mut self,
        channel: WifiChannel,
        observer: &mut O,
    ) -> Result<(), Esp32s31MonitorChannelSwitchError>
    where
        D: PhyAsyncDelay,
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
            + open_esp_radio_esp32s31_hal::phy_temperature::PhyTemperatureSystemControl
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
        O: PhyTargetObserver,
    {
        self.owner.switch_channel::<D, O>(channel, observer).await
    }
}

/// Materialize one executor-owned task and its hardware-free application
/// controller in a single transaction.
#[allow(clippy::type_complexity)]
pub fn prepare_esp32s31_monitor_task<
    'runtime,
    P,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    plan: WifiStandaloneMonitorPlan,
    wifi: Esp32s31WifiStopped<P>,
    resources: Esp32s31MonitorTaskResources<
        'runtime,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
) -> Result<
    (
        Esp32s31MonitorController<'runtime, M>,
        Esp32s31MonitorTask<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ),
    Esp32s31MonitorTaskBuildFailure<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
>
where
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    let Esp32s31MonitorTaskResources { runtime, control } = resources;
    if let Err(error) = control.ensure_reusable() {
        return Err(Esp32s31MonitorTaskBuildFailure {
            error: Esp32s31MonitorBuildError::Control(error),
            plan,
            wifi,
            resources: Esp32s31MonitorTaskResources { runtime, control },
        });
    }
    match prepare_esp32s31_monitor(plan, wifi, runtime) {
        Ok(owner) => {
            let (controller, receiver) = control.split_checked();
            Ok((
                controller,
                Esp32s31MonitorTask {
                    owner,
                    control: receiver,
                },
            ))
        }
        Err(failure) => Err(Esp32s31MonitorTaskBuildFailure {
            error: failure.error,
            plan: failure.plan,
            wifi: failure.wifi,
            resources: Esp32s31MonitorTaskResources {
                runtime: failure.resources,
                control,
            },
        }),
    }
}
