//! Bounded standalone-STA RX producer/consumer owner.
//!
//! The physical DMA producer and connected protocol processor deliberately
//! live in one DATAPATH service. A turn first consumes already staged leases,
//! refills the hardware ring once, then consumes newly staged leases with the
//! remaining frame credit. This preserves the finite DMA frontier while
//! removing the executor task and `select` boundary between the two phases.

use core::future::Future;

use open_esp_radio_embassy_net::RawMutex;

use crate::{
    datapath::{
        DatapathRxProgress, DatapathRxServiceContext, DatapathRxWorkCounters,
        network::DatapathNetworkRxSet, services::DatapathRxService,
    },
    roles::station::{
        rx_protocol::{
            ConnectedRxProtocolSink, Esp32s31ConnectedRxProtocol,
            Esp32s31ConnectedRxProtocolStopped,
        },
        teardown::Esp32s31ConnectedStaRxTeardown,
    },
};

const ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES: usize = 4;

/// One standalone-STA RX owner spanning DMA staging and 802.11 processing.
pub struct Esp32s31ConnectedStaRxService<R, P> {
    dma: R,
    protocol: P,
    serviced_frames: u64,
}

/// Reusable owners returned only after both DMA and protocol state stop.
pub struct Esp32s31ConnectedStaRxStopped<R, P> {
    dma: R,
    protocol: P,
}

impl<R, P> Esp32s31ConnectedStaRxStopped<R, P> {
    pub fn into_parts(self) -> (R, P) {
        (self.dma, self.protocol)
    }
}

impl<R, P> Esp32s31ConnectedStaRxService<R, P> {
    pub const fn new(dma: R, protocol: P) -> Self {
        Self {
            dma,
            protocol,
            serviced_frames: 0,
        }
    }

    pub const fn dma(&self) -> &R {
        &self.dma
    }

    pub fn dma_mut(&mut self) -> &mut R {
        &mut self.dma
    }

    pub const fn protocol(&self) -> &P {
        &self.protocol
    }

    pub fn protocol_mut(&mut self) -> &mut P {
        &mut self.protocol
    }

    pub const fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    pub fn into_parts(self) -> (R, P) {
        (self.dma, self.protocol)
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M,
    H,
    R,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> Esp32s31ConnectedStaRxTeardown<H>
    for Esp32s31ConnectedStaRxService<
        R,
        Esp32s31ConnectedRxProtocol<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            DEPTH,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    >
where
    M: RawMutex,
    R: Esp32s31ConnectedStaRxTeardown<H>,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    type Stopped = Esp32s31ConnectedStaRxStopped<
        R::Stopped,
        Esp32s31ConnectedRxProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    >;
    type Error = R::Error;

    fn try_stop(self, hardware: &mut H) -> Result<Self::Stopped, (Self, Self::Error)> {
        let Self {
            dma,
            protocol,
            serviced_frames,
        } = self;
        match dma.try_stop(hardware) {
            Ok(dma) => Ok(Esp32s31ConnectedStaRxStopped {
                dma,
                protocol: protocol.into_stopped(),
            }),
            Err((dma, error)) => Err((
                Self {
                    dma,
                    protocol,
                    serviced_frames,
                },
                error,
            )),
        }
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M,
    H,
    R,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> DatapathRxService<H>
    for Esp32s31ConnectedStaRxService<
        R,
        Esp32s31ConnectedRxProtocol<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            DEPTH,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    >
where
    M: RawMutex,
    R: DatapathRxService<H>,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    type Error = R::Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.dma.service(hardware)
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        self.dma.work_counters()
    }

    async fn service_turn<'a>(
        &'a mut self,
        hardware: &'a mut H,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, Self::Error> {
        let limit = context.maximum_protocol_frames.unwrap_or(SLOTS).max(1);
        let before_dma = if self.protocol.has_ready_work() {
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            let protocol_started =
                crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
            #[cfg(feature = "task-poll-telemetry")]
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .begin_protocol_poll(protocol_started.cycles);
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .begin_protocol_poll(protocol_started);
            let turn = self.protocol.service_bounded(limit).await;
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .record_protocol_paths(turn.direct_frames, turn.asynchronous_frames);
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            {
                let protocol_ended =
                    crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
                #[cfg(feature = "task-poll-telemetry")]
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                    .end_protocol_poll(protocol_ended.cycles);
                crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                    .end_protocol_poll(protocol_ended);
            }
            turn
        } else {
            Default::default()
        };
        self.serviced_frames = self
            .serviced_frames
            .saturating_add(before_dma.consumed_frames as u64);

        // Refill once even when the protocol budget was consumed by an
        // existing backlog. The leases just released above are the exact
        // credits needed to keep the hardware ring from becoming the only
        // remaining ingress reserve.
        let dma_progress = self.dma.service(hardware).await?;

        let remaining = limit.saturating_sub(before_dma.consumed_frames);
        let after_dma = if remaining == 0 || !self.protocol.has_ready_work() {
            Default::default()
        } else {
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            let protocol_started =
                crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
            #[cfg(feature = "task-poll-telemetry")]
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .begin_protocol_poll(protocol_started.cycles);
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .begin_protocol_poll(protocol_started);
            let turn = self.protocol.service_bounded(remaining).await;
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .record_protocol_paths(turn.direct_frames, turn.asynchronous_frames);
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            {
                let protocol_ended =
                    crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
                #[cfg(feature = "task-poll-telemetry")]
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                    .end_protocol_poll(protocol_ended.cycles);
                crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                    .end_protocol_poll(protocol_ended);
            }
            turn
        };
        self.serviced_frames = self
            .serviced_frames
            .saturating_add(after_dma.consumed_frames as u64);

        // `StageCapacityBlocked` describes the capacity observed inside the
        // DMA phase, not necessarily the state at this fused turn boundary.
        // A successful after-DMA protocol pass returns at least one staging
        // credit synchronously. Preserve the known completed DMA frontier as
        // runnable work instead of sleeping for a capacity edge which already
        // happened inside this owner.
        let stage_capacity_released = dma_progress == DatapathRxProgress::StageCapacityBlocked
            && after_dma.consumed_frames != 0;
        Ok(
            if before_dma.work_remaining
                || after_dma.work_remaining
                || self.protocol.has_ready_work()
                || stage_capacity_released
            {
                DatapathRxProgress::BudgetExhausted
            } else {
                dma_progress
            },
        )
    }

    fn service_turn_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.service_turn(
            hardware,
            network_rx,
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES),
            },
        )
    }

    fn has_work(&self) -> bool {
        self.protocol.has_ready_work()
    }

    fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.protocol.wait_ready()
    }
}
