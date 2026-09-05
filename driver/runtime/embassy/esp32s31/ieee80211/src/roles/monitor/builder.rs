//! One-shot materialization of a standalone ESP32-S31 monitor owner graph.

#![forbid(unsafe_code)]

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::Timer;
use open_esp_radio_esp32s31_hal::{MacInterruptEnableState, RadioRuntimeOwner};
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi::{
    mac_start::Esp32s31WifiMacStartReport,
    runtime::{Esp32s31WifiRoleOwner, Esp32s31WifiRuntimeContext},
    switch_esp32s31_wifi_channel,
};
use open_esp_radio_esp32s31_wifi_dma::rx_storage::RxDmaStorageError;
use open_esp_radio_esp32s31_wifi_mac::{
    irq::MacInterruptRoute,
    rx::{RxDmaBufferAddresses, RxPhyInfo, RxRingHalted, RxRingLive},
};
use open_esp_radio_ieee80211::channel::WifiChannel;
use open_esp_radio_wifi_softmac::{
    MonitorChannelPolicy, MonitorChannelSequence, MonitorSink, WifiStandaloneMonitorPlan,
};

use crate::{
    datapath::irq::Esp32s31MacInterruptEpoch,
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
    buffer_addresses: &'static RxDmaBufferAddresses<COUNT>,
    descriptor_base: u32,
}

impl<const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub fn new(
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        buffer_addresses: &'static mut RxDmaBufferAddresses<COUNT>,
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

    pub const fn buffer_addresses(self) -> &'static RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    pub const fn descriptor_base(self) -> u32 {
        self.descriptor_base
    }
}

/// Role-neutral physical radio frontier consumed by a monitor task.
///
/// The interrupt epoch is deliberately already materialized. Logical role
/// changes never uninstall its CPU route while the MAC remains powered.
pub struct Esp32s31MonitorRadio<'runtime, P, R, M: RawMutex>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
{
    pub owner: Esp32s31WifiRoleOwner<P>,
    pub registers: RadioRuntimeOwner,
    pub interrupts: Esp32s31MacInterruptEpoch<'runtime, R, M>,
}

impl<'runtime, P, R, M: RawMutex> Esp32s31MonitorRadio<'runtime, P, R, M>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
{
    pub fn new(
        owner: Esp32s31WifiRoleOwner<P>,
        registers: RadioRuntimeOwner,
        interrupts: Esp32s31MacInterruptEpoch<'runtime, R, M>,
    ) -> Self {
        Self {
            owner,
            registers,
            interrupts,
        }
    }
}

/// Physical RX-ring authority accepted by a monitor role.
///
/// `Halted` exists only for the first cold materialization. Every logical
/// role handoff after the walker has started carries `Live` unchanged.
pub enum Esp32s31MonitorRxRing<const COUNT: usize> {
    Halted(RxRingHalted<'static, COUNT>),
    Live(RxRingLive<'static, COUNT>),
}

/// Runtime-owned resources consumed together when a monitor role starts.
///
/// The frame arena and address table are board placement policy. The route,
/// wake runtimes and sink are executor integration. Grouping them prevents a
/// caller from accidentally pairing an RX ring with a different IRQ epoch.
struct Esp32s31MonitorResources<
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    dma: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    rx_ring: Option<Esp32s31MonitorRxRing<COUNT>>,
    sink: S,
}

impl<S, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    const fn new(
        dma: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        rx_ring: Option<Esp32s31MonitorRxRing<COUNT>>,
        sink: S,
    ) -> Self {
        Self { dma, rx_ring, sink }
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
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: Esp32s31MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub radio: Esp32s31MonitorRadio<'runtime, P, R, M>,
    pub resources: Esp32s31MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31MonitorBuildReport {
    pub start: Esp32s31WifiMacStartReport,
    pub cold_interrupt_mask: MacInterruptEnableState,
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
    R: MacInterruptRoute<Platform = P>,
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
    R: MacInterruptRoute<Platform = P>,
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

    /// Return the common live radio owner and every reusable monitor resource
    /// after the logical monitor consumer is parked.
    #[expect(
        clippy::result_large_err,
        reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
    )]
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
        let live_ring = receive
            .into_live()
            .unwrap_or_else(|_| unreachable!("a decomposable monitor service retains live RX"));
        let radio = Esp32s31MonitorRadio::new(
            Esp32s31WifiRoleOwner::from_runtime_parts(platform, context),
            registers,
            interrupts,
        );
        Ok(Esp32s31MonitorStopped {
            radio,
            plan,
            resources: Esp32s31MonitorStoppedResources {
                memory,
                live_ring,
                sink,
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
#[expect(
    clippy::result_large_err,
    reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
)]
#[expect(
    clippy::type_complexity,
    reason = "the return type preserves the concrete radio, sink and DMA resource dimensions"
)]
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
    mut radio: Esp32s31MonitorRadio<'runtime, P, R, M>,
    mut resources: Esp32s31MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
) -> Result<
    Esp32s31MonitorOwner<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    Esp32s31MonitorBuildFailure<'runtime, P, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    let cold_interrupt_mask = radio.owner.transition_report().cold_interrupt_mask;
    let initial_channel = radio.owner.current_channel();
    let receive = {
        let mut registers = radio.registers.wifi_mac_hal();
        match resources.rx_ring.take() {
            Some(Esp32s31MonitorRxRing::Halted(ring)) => {
                match Esp32s31MonitorRx::prepare_halted(
                    plan,
                    ring,
                    &mut registers,
                    resources.dma.storage,
                ) {
                    Ok(receive) => Ok(receive),
                    Err((ring, error)) => {
                        resources.rx_ring = Some(Esp32s31MonitorRxRing::Halted(ring));
                        Err(error)
                    }
                }
            }
            Some(Esp32s31MonitorRxRing::Live(ring)) => {
                match Esp32s31MonitorRx::from_live(plan, ring, resources.dma.storage) {
                    Ok(receive) => Ok(receive),
                    Err((ring, error)) => {
                        resources.rx_ring = Some(Esp32s31MonitorRxRing::Live(ring));
                        Err(error)
                    }
                }
            }
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
                radio,
                resources,
            });
        }
    };
    let start = radio.owner.start_report();
    let (platform, context) = radio.owner.into_runtime_parts();
    let mut service = Esp32s31MonitorService::new(
        radio.registers,
        receive,
        resources.sink,
        radio.interrupts,
        platform,
    );
    service
        .begin_channel_epoch(initial_channel)
        .expect("a newly prepared monitor service is quiescent");
    Ok(Esp32s31MonitorOwner {
        service,
        context,
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
/// capture publication and control storage move together. Physical interrupt
/// ownership belongs to [`Esp32s31MonitorRadio`], not this logical role.
pub struct Esp32s31MonitorTaskResources<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    runtime: Esp32s31MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    control: &'runtime Esp32s31MonitorControlResources<M>,
}

impl<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorTaskResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    pub fn new(
        memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        rx_ring: Option<Esp32s31MonitorRxRing<COUNT>>,
        sink: S,
        control: &'runtime Esp32s31MonitorControlResources<M>,
    ) -> Self {
        Self {
            runtime: Esp32s31MonitorResources::new(memory, rx_ring, sink),
            control,
        }
    }
}

/// Reusable board/executor resources returned by a stopped monitor role.
///
/// Physical interrupt ownership is returned separately in the common radio
/// frontier, so these resources remain purely role-local.
pub struct Esp32s31MonitorStoppedResources<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    live_ring: RxRingLive<'static, COUNT>,
    sink: S,
    control: &'runtime Esp32s31MonitorControlResources<M>,
}

/// Named role-local owner set returned when monitor resources are rebound to a
/// different Wi-Fi role by a supervisor or qualification harness.
pub struct Esp32s31MonitorStoppedResourceParts<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    pub memory: Esp32s31MonitorMemory<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub live_ring: RxRingLive<'static, COUNT>,
    pub sink: S,
    pub control: &'runtime Esp32s31MonitorControlResources<M>,
}

impl<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31MonitorStoppedResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    pub fn into_parts(
        self,
    ) -> Esp32s31MonitorStoppedResourceParts<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
    {
        Esp32s31MonitorStoppedResourceParts {
            memory: self.memory,
            live_ring: self.live_ring,
            sink: self.sink,
            control: self.control,
        }
    }

    /// Rebind the exact returned role-local resources to another monitor task.
    pub fn into_task_resources(
        self,
    ) -> Esp32s31MonitorTaskResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
    {
        Esp32s31MonitorTaskResources::new(
            self.memory,
            Some(Esp32s31MonitorRxRing::Live(self.live_ring)),
            self.sink,
            self.control,
        )
    }
}

/// Fully dematerialized standalone monitor role.
///
/// This value can exist only after the task parked its logical consumer and
/// returned the live RX ring. `radio` retains the installed physical IRQ
/// epoch for the next role.
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
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub radio: Esp32s31MonitorRadio<'runtime, P, R, M>,
    pub plan: WifiStandaloneMonitorPlan,
    pub resources:
        Esp32s31MonitorStoppedResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
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
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: Esp32s31MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub radio: Esp32s31MonitorRadio<'runtime, P, R, M>,
    pub resources:
        Esp32s31MonitorTaskResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
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
    R: MacInterruptRoute<Platform = P>,
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
    R: MacInterruptRoute<Platform = P>,
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
    R: MacInterruptRoute<Platform = P>,
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
    #[expect(
        clippy::result_large_err,
        reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
    )]
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
        O: PhyTargetObserver,
    {
        self.owner.switch_channel::<D, O>(channel, observer).await
    }
}

/// Materialize one executor-owned task and its hardware-free application
/// controller in a single transaction.
#[allow(clippy::type_complexity)]
#[expect(
    clippy::result_large_err,
    reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
)]
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
    radio: Esp32s31MonitorRadio<'runtime, P, R, M>,
    resources: Esp32s31MonitorTaskResources<
        'runtime,
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
    R: MacInterruptRoute<Platform = P>,
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
                radio,
                resources: Esp32s31MonitorTaskResources { runtime, control },
            });
        }
    };
    match prepare_esp32s31_monitor(plan, radio, runtime) {
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
                radio: failure.radio,
                resources: Esp32s31MonitorTaskResources {
                    runtime: failure.resources,
                    control,
                },
            })
        }
    }
}
