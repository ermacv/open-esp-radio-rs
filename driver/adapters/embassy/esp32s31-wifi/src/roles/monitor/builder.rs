//! One-shot materialization of a standalone ESP32-S31 monitor owner graph.

#![forbid(unsafe_code)]

use embassy_futures::select::{Either, select};
use embassy_time::Timer;
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_hal::{MacInterruptSetup, RadioRuntimeOwner};
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi::{
    mac_start::Esp32s31WifiMacStartReport,
    runtime::{Esp32s31WifiRuntimeContext, Esp32s31WifiStopped},
    switch_esp32s31_wifi_channel,
};
use open_esp_radio_esp32s31_wifi_dma::rx_storage::RxDmaStorageError;
use open_esp_radio_esp32s31_wifi_mac::{
    init::activate_promiscuous_receive,
    irq::MacInterruptRoute,
    rx::{RxPhyInfo, RxRingHalted},
};
use open_esp_radio_ieee80211::channel::WifiChannel;
use open_esp_radio_wifi_softmac::{
    MonitorChannelPolicy, MonitorChannelSequence, MonitorSink, WifiStandaloneMonitorPlan,
};

use crate::{
    datapath::irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch},
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::Esp32s31RxFrontierError,
    roles::monitor::rx::{Esp32s31MonitorPrepareError, Esp32s31MonitorRx},
    roles::monitor::service::{
        Esp32s31MonitorRunError, Esp32s31MonitorRunFailure, Esp32s31MonitorRunReport,
        Esp32s31MonitorService, Esp32s31MonitorStoppedAccessError,
    },
    roles::monitor::{
        Esp32s31MonitorCommandReceiver, Esp32s31MonitorCompletion, Esp32s31MonitorControlError,
        Esp32s31MonitorControlResources, Esp32s31MonitorController,
    },
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

    /// Reborrow the common RX arena for another role-local stopped resource
    /// graph. A physical supervisor must still enforce exclusive activation.
    pub const fn storage(
        self,
    ) -> &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub const fn buffer_addresses(self) -> &'static [u32; COUNT] {
        self.buffer_addresses
    }

    pub const fn descriptor_base(self) -> u32 {
        self.descriptor_base
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

/// Exact platform route and wake runtimes returned by a stopped monitor.
pub struct Esp32s31MonitorInterruptParts<'runtime, R, M: RawMutex>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    R::Platform: Sized,
{
    pub route: R,
    pub mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    pub power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
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

    pub fn into_parts(self) -> Esp32s31MonitorInterruptParts<'runtime, R, M> {
        Esp32s31MonitorInterruptParts {
            route: self.route,
            mac_runtime: self.mac_runtime,
            power_runtime: self.power_runtime,
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
    halted_ring: Option<RxRingHalted<'static, COUNT>>,
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
        halted_ring: Option<RxRingHalted<'static, COUNT>>,
        sink: S,
        route: R,
        mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
        power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    ) -> Self {
        Self {
            dma,
            halted_ring,
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
    Receive(Esp32s31RxFrontierError),
    Quarantined,
    PolicyMismatch {
        expected: WifiChannel,
        active: WifiChannel,
    },
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
        RadioRuntimeOwner,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    context: Esp32s31WifiRuntimeContext,
    report: Esp32s31MonitorBuildReport,
    plan: WifiStandaloneMonitorPlan,
    memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    capture_channel: WifiChannel,
    quarantined: bool,
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

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }

    pub const fn capture_channel(&self) -> WifiChannel {
        self.capture_channel
    }

    /// Return the common Wi-Fi owner and every reusable monitor resource only
    /// after IRQ routing and RX DMA are both proven inactive.
    fn try_into_stopped(
        self,
        control: &'runtime Esp32s31MonitorControlResources<M>,
    ) -> Result<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
    > {
        if self.quarantined || self.service.is_quarantined() {
            return Err(self);
        }
        let Self {
            service,
            context,
            report,
            plan,
            memory,
            capture_channel,
            quarantined,
        } = self;
        let (registers, receive, sink, interrupts, platform) = match service.try_into_parts() {
            Ok(parts) => parts,
            Err(service) => {
                return Err(Self {
                    service,
                    context,
                    report,
                    plan,
                    memory,
                    capture_channel,
                    quarantined,
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
        let halted_ring = receive
            .into_halted()
            .unwrap_or_else(|_| unreachable!("a decomposable monitor service has stopped RX"));
        let wifi = context.into_stopped(platform, registers, interrupt_setup);
        Ok(Esp32s31MonitorStopped {
            wifi,
            plan,
            resources: Esp32s31MonitorStoppedResources {
                memory,
                halted_ring,
                sink,
                interrupts: Esp32s31MonitorInterrupts::new(route, mac_runtime, power_runtime),
                control,
            },
        })
    }

    /// Run inside the long-lived role task until its application handle asks
    /// for shutdown. Completion is published only after IRQ and DMA are
    /// confirmed quiescent, or as `Faulted` while this task still retains every
    /// hardware-visible owner in quarantine for explicit board recovery.
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
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
        O: PhyTargetObserver,
    {
        if self.quarantined {
            return Err(Esp32s31MonitorChannelSwitchError::Quarantined);
        }
        self.service
            .begin_stopped_transition()
            .map_err(Esp32s31MonitorChannelSwitchError::Active)?;
        self.quarantined = true;
        let (registers, platform) = self
            .service
            .stopped_radio_mut()
            .map_err(Esp32s31MonitorChannelSwitchError::Active)?;
        let switched = switch_esp32s31_wifi_channel::<D, _, _>(
            self.context.phy_mut(),
            channel,
            platform,
            registers,
            observer,
        )
        .await;
        if let Err(error) = switched {
            self.quarantined = true;
            return Err(Esp32s31MonitorChannelSwitchError::Phy(error));
        }
        activate_promiscuous_receive(registers);
        self.context.set_current_channel(channel);
        if let Err(error) = self.service.prepare_next_receive_epoch() {
            self.quarantined = true;
            return Err(Esp32s31MonitorChannelSwitchError::Receive(error));
        }
        if let Err(error) = self.service.begin_channel_epoch(channel) {
            self.quarantined = true;
            return Err(Esp32s31MonitorChannelSwitchError::Active(error));
        }
        self.capture_channel = channel;
        self.service.complete_stopped_transition();
        self.quarantined = false;
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
    mut resources: Esp32s31MonitorResources<
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
    let initial_channel = wifi.current_channel();
    let receive = {
        let (mut registers, _) = wifi.radio_mut();
        activate_promiscuous_receive(&mut registers);
        match resources.halted_ring.take() {
            Some(ring) => match Esp32s31MonitorRx::prepare_halted(
                plan,
                ring,
                &mut registers,
                resources.dma.storage,
            ) {
                Ok(receive) => Ok(receive),
                Err((ring, error)) => {
                    resources.halted_ring = Some(ring);
                    Err(error)
                }
            },
            None => Esp32s31MonitorRx::prepare_initial(
                plan,
                &mut registers,
                resources.dma.storage,
                resources.dma.descriptor_base,
                resources.dma.buffer_addresses,
            ),
        }
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
    let interrupts = Esp32s31MacInterruptEpoch::new(
        resources.route,
        runtime.interrupt_setup,
        resources.mac_runtime,
        resources.power_runtime,
    );
    let mut service = Esp32s31MonitorService::new(
        runtime.registers,
        receive,
        resources.sink,
        interrupts,
        runtime.platform,
    );
    service
        .begin_channel_epoch(initial_channel)
        .expect("a newly prepared monitor service is quiescent");
    Ok(Esp32s31MonitorOwner {
        service,
        context: runtime.context,
        report: Esp32s31MonitorBuildReport {
            start,
            cold_interrupt_mask,
            descriptor_base: resources.dma.descriptor_base,
        },
        plan,
        memory: resources.dma,
        capture_channel: initial_channel,
        quarantined: false,
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
    control: &'runtime Esp32s31MonitorControlResources<M>,
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
        halted_ring: Option<RxRingHalted<'static, COUNT>>,
        sink: S,
        interrupts: Esp32s31MonitorInterrupts<'runtime, R, M>,
        control: &'runtime Esp32s31MonitorControlResources<M>,
    ) -> Self {
        Self {
            runtime: Esp32s31MonitorResources::new(
                memory,
                halted_ring,
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
    halted_ring: RxRingHalted<'static, COUNT>,
    sink: S,
    interrupts: Esp32s31MonitorInterrupts<'runtime, R, M>,
    control: &'runtime Esp32s31MonitorControlResources<M>,
}

/// Named role-local owner set returned when monitor resources are rebound to a
/// different Wi-Fi role by a supervisor or qualification harness.
pub struct Esp32s31MonitorStoppedResourceParts<
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
    pub memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub halted_ring: RxRingHalted<'static, COUNT>,
    pub sink: S,
    pub interrupts: Esp32s31MonitorInterrupts<'runtime, R, M>,
    pub control: &'runtime Esp32s31MonitorControlResources<M>,
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
    pub fn into_parts(
        self,
    ) -> Esp32s31MonitorStoppedResourceParts<
        'runtime,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        Esp32s31MonitorStoppedResourceParts {
            memory: self.memory,
            halted_ring: self.halted_ring,
            sink: self.sink,
            interrupts: self.interrupts,
            control: self.control,
        }
    }

    /// Rebind the exact returned role-local resources to another monitor task.
    pub fn into_task_resources(
        self,
    ) -> Esp32s31MonitorTaskResources<'runtime, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
    {
        Esp32s31MonitorTaskResources::new(
            self.memory,
            Some(self.halted_ring),
            self.sink,
            self.interrupts,
            self.control,
        )
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

/// Terminal output of one consuming standalone-monitor task epoch.
///
/// A returned stopped owner proves that IRQ routing and RX DMA are both
/// inactive. `Faulted` deliberately retains the complete task at its exact
/// hardware frontier; callers may only hand it to reset policy and cannot
/// misclassify a run error as reusable stopped Wi-Fi.
pub enum Esp32s31MonitorTaskExit<S, T, E> {
    Stopped {
        stopped: S,
        result: Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<E>>,
    },
    Faulted {
        task: T,
        result: Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<E>>,
    },
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
    fn policy_mismatch(&mut self, expected: WifiChannel) -> Esp32s31MonitorRunFailure<R::Error> {
        let active = self.owner.capture_channel();
        self.control.complete(Esp32s31MonitorCompletion::Stopped);
        Esp32s31MonitorRunFailure {
            error: Esp32s31MonitorRunError::Channel(
                Esp32s31MonitorChannelSwitchError::PolicyMismatch { expected, active },
            ),
            report: Esp32s31MonitorRunReport::default(),
        }
    }

    async fn run_hopping<D, O>(
        &mut self,
        sequence: MonitorChannelSequence,
        observer: &mut O,
    ) -> Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<R::Error>>
    where
        D: PhyAsyncDelay,
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
        O: PhyTargetObserver,
    {
        if self.owner.current_channel() != sequence.first()
            || self.owner.capture_channel() != sequence.first()
        {
            return Err(self.policy_mismatch(sequence.first()));
        }

        let mut report = Esp32s31MonitorRunReport::default();
        let channels = sequence.channels();
        let mut channel_index = 0_usize;
        loop {
            if self.control.stop_requested() {
                self.control.complete(Esp32s31MonitorCompletion::Stopped);
                return Ok(report);
            }

            let epoch = {
                let owner = &mut self.owner;
                let control = &mut self.control;
                owner
                    .service
                    .run_until_boundary(select(
                        control.wait_stop(),
                        Timer::after_millis(u64::from(sequence.dwell_millis())),
                    ))
                    .await
            };
            let (epoch, boundary) = match epoch {
                Ok(completed) => completed,
                Err(failure) => {
                    report.merge(failure.report);
                    self.control.complete(
                        if self.owner.service.is_quiescent()
                            && !self.owner.quarantined
                            && !self.owner.service.is_quarantined()
                        {
                            Esp32s31MonitorCompletion::Stopped
                        } else {
                            Esp32s31MonitorCompletion::Faulted
                        },
                    );
                    return Err(Esp32s31MonitorRunFailure {
                        error: failure.error,
                        report,
                    });
                }
            };
            report.merge(epoch);
            match boundary {
                Either::First(()) => {
                    self.control.complete(Esp32s31MonitorCompletion::Stopped);
                    return Ok(report);
                }
                Either::Second(_) => {}
            }

            // A stop published while the dwell boundary was being closed wins
            // before another stopped-only retune or RX epoch begins.
            if self.control.stop_requested() {
                self.control.complete(Esp32s31MonitorCompletion::Stopped);
                return Ok(report);
            }
            channel_index = (channel_index + 1) % channels.len();
            let next_channel = channels[channel_index];
            if let Err(error) = self
                .owner
                .switch_channel::<D, O>(next_channel, observer)
                .await
            {
                self.owner.quarantined = true;
                self.owner.service.force_quarantine();
                self.control.complete(Esp32s31MonitorCompletion::Faulted);
                return Err(Esp32s31MonitorRunFailure {
                    error: Esp32s31MonitorRunError::Channel(error),
                    report,
                });
            }
            report.channel_switches = report.channel_switches.saturating_add(1);
        }
    }
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

    pub const fn current_channel(&self) -> WifiChannel {
        self.owner.current_channel()
    }

    /// Reach the current S31 injection frontier through this task's actual
    /// capture dwell. The present backend fails at its unassigned monitor TX
    /// interface before borrowing sequence, DMA or IRQ state.
    pub fn admit_injection_frontier<const TX_BUFFER_SIZE: usize>(
        &self,
        request: open_esp_radio_wifi_softmac::MonitorInjectionRequest<'_>,
    ) -> Result<
        open_esp_radio_esp32s31_wifi::monitor_injection::Esp32s31MonitorInjectionAdmission,
        open_esp_radio_esp32s31_wifi::monitor_injection::Esp32s31MonitorInjectionAdmissionError,
    > {
        self.owner
            .service
            .admit_injection::<TX_BUFFER_SIZE>(self.owner.plan, request)
    }

    /// Run the finite role epoch. The task-side command receiver is private,
    /// so only the paired controller can request the terminal edge.
    pub async fn run(
        &mut self,
    ) -> Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<R::Error>> {
        self.owner.run_controlled(&mut self.control).await
    }

    /// Run and consume one complete role epoch for a supervisor task wrapper.
    ///
    /// The run result and hardware ownership are intentionally independent:
    /// an operational error can still leave a safely stopped owner, while an
    /// apparently completed control exchange must not permit reuse if the
    /// IRQ/DMA owners did not return.
    #[allow(clippy::type_complexity)]
    pub async fn run_to_exit(
        mut self,
    ) -> Esp32s31MonitorTaskExit<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
        R::Error,
    > {
        let result = self.run().await;
        match self.try_into_stopped() {
            Ok(stopped) => Esp32s31MonitorTaskExit::Stopped { stopped, result },
            Err(task) => Esp32s31MonitorTaskExit::Faulted { task, result },
        }
    }

    /// Run either the compatible fixed-channel epoch or one bounded hopping
    /// cycle until the paired controller requests stop.
    #[allow(clippy::type_complexity)]
    pub async fn run_channel_policy_to_exit<D, O>(
        mut self,
        policy: MonitorChannelPolicy,
        observer: &mut O,
    ) -> Esp32s31MonitorTaskExit<
        Esp32s31MonitorStopped<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
        R::Error,
    >
    where
        D: PhyAsyncDelay,
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
        O: PhyTargetObserver,
    {
        let result = match policy {
            MonitorChannelPolicy::Fixed(channel)
                if self.owner.current_channel() != channel
                    || self.owner.capture_channel() != channel =>
            {
                Err(self.policy_mismatch(channel))
            }
            MonitorChannelPolicy::Fixed(_) => self.run().await,
            MonitorChannelPolicy::Hopping(sequence) => {
                self.run_hopping::<D, O>(sequence, observer).await
            }
        };
        match self.try_into_stopped() {
            Ok(stopped) => Esp32s31MonitorTaskExit::Stopped { stopped, result },
            Err(task) => Esp32s31MonitorTaskExit::Faulted { task, result },
        }
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
        let control_resources = control.resources();
        match owner.try_into_stopped(control_resources) {
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
    let (controller, mut receiver) = match control.split() {
        Ok(endpoints) => endpoints,
        Err(error) => {
            return Err(Esp32s31MonitorTaskBuildFailure {
                error: Esp32s31MonitorBuildError::Control(error),
                plan,
                wifi,
                resources: Esp32s31MonitorTaskResources { runtime, control },
            });
        }
    };
    match prepare_esp32s31_monitor(plan, wifi, runtime) {
        Ok(owner) => Ok((
            controller,
            Esp32s31MonitorTask {
                owner,
                control: receiver,
            },
        )),
        Err(failure) => {
            // No task or live IRQ/DMA epoch was created. Return the control
            // lease cleanly so the exact build resources remain retryable.
            receiver.complete(Esp32s31MonitorCompletion::Stopped);
            drop(receiver);
            drop(controller);
            Err(Esp32s31MonitorTaskBuildFailure {
                error: failure.error,
                plan: failure.plan,
                wifi: failure.wifi,
                resources: Esp32s31MonitorTaskResources {
                    runtime: failure.resources,
                    control,
                },
            })
        }
    }
}
