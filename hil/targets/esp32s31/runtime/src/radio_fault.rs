//! HIL-only fault adapter around the production connected radio services.
//!
//! The adapter never fabricates a station/lifecycle result. TX injection first
//! lets the real services publish a MAC descriptor, then supplies an impossible
//! completion/timeout image. RX injection narrows admission only after the
//! production owner has taken a real completed DMA unit and observes recovery
//! only after reload plus a following staged unit.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use open_esp_radio::adapters::esp32s31::wifi_embassy::connected_runner::{
    ConnectedRunnerServices, WifiControlContext, WifiControlProgress, WifiRxProgress,
    WifiTxProgress, WifiTxWake,
};
use open_esp_radio::adapters::esp32s31::wifi_embassy::rx_dma_service::{
    Esp32s31RxCompletedUnit, Esp32s31RxIngressObservation, Esp32s31RxStageAdmissionPolicy,
};
use open_esp_radio::adapters::esp32s31::wifi_embassy::rx_pipeline_observer::RxStageDiscard;
use open_esp_radio::adapters::network::embassy_net::{PinnedTxConsumer, PinnedTxFrame, RawMutex};
use open_esp_radio::esp32s31::wifi::lmac::irq::{MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT};
use open_esp_radio_hil_protocol::{
    StationFaultClassification, StationFaultEvidence, StationFaultInjection,
};

const FAULT_IDLE: u8 = 0;
const FAULT_ARMING: u8 = 1;
const TX_FAULT_ARMED: u8 = 2;
const TX_FAULT_ACTIVE: u8 = 3;
const RX_FAULT_ARMED: u8 = 4;
const RX_FAULT_DISCARD_PENDING: u8 = 5;
const RX_FAULT_WAITING_FOR_STAGED_UNIT: u8 = 6;
const RX_FAULT_COMPLETE: u8 = 7;

/// Correlation retained from the host request to the selected fault frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArmedStationFault {
    pub request_id: u32,
    pub injection: StationFaultInjection,
}

/// Single one-shot fault cell shared by the protocol and radio tasks.
pub struct StationFaultControl {
    state: AtomicU8,
    request_id: AtomicU32,
}

impl StationFaultControl {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(FAULT_IDLE),
            request_id: AtomicU32::new(0),
        }
    }

    /// Arm one fault only while no earlier request is pending or active.
    pub fn try_arm(&self, request_id: u32, injection: StationFaultInjection) -> bool {
        if self
            .state
            .compare_exchange(
                FAULT_IDLE,
                FAULT_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        self.request_id.store(request_id, Ordering::Relaxed);
        self.state.store(
            match injection {
                StationFaultInjection::ConnectedTxAfterPublication => TX_FAULT_ARMED,
                StationFaultInjection::ConnectedRxBeforeStagingOverCapacity => RX_FAULT_ARMED,
            },
            Ordering::Release,
        );
        true
    }

    fn take_after_tx_publication(&self) -> Option<ArmedStationFault> {
        self.state
            .compare_exchange(
                TX_FAULT_ARMED,
                TX_FAULT_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        Some(ArmedStationFault {
            request_id: self.request_id.load(Ordering::Relaxed),
            injection: StationFaultInjection::ConnectedTxAfterPublication,
        })
    }

    fn take_recovered_rx_fault(&self) -> Option<ArmedStationFault> {
        self.state
            .compare_exchange(
                RX_FAULT_COMPLETE,
                FAULT_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        let fault = ArmedStationFault {
            request_id: self.request_id.load(Ordering::Relaxed),
            injection: StationFaultInjection::ConnectedRxBeforeStagingOverCapacity,
        };
        self.state.store(FAULT_IDLE, Ordering::Release);
        Some(fault)
    }
}

impl Default for StationFaultControl {
    fn default() -> Self {
        Self::new()
    }
}

impl Esp32s31RxStageAdmissionPolicy for StationFaultControl {
    fn maximum_payload_length(
        &self,
        unit: Esp32s31RxCompletedUnit,
        physical_capacity: usize,
    ) -> usize {
        if unit.payload_length != 0
            && self
                .state
                .compare_exchange(
                    RX_FAULT_ARMED,
                    RX_FAULT_DISCARD_PENDING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            // A zero admission limit makes this real, non-empty completed unit
            // cross the production TooLong recycle path without mutating its
            // descriptor or fabricating a second DMA owner.
            0
        } else {
            physical_capacity
        }
    }

    fn observe(&self, observation: Esp32s31RxIngressObservation) {
        match observation {
            Esp32s31RxIngressObservation::DiscardReloaded {
                reason: RxStageDiscard::TooLong,
                ..
            } => {
                let _ = self.state.compare_exchange(
                    RX_FAULT_DISCARD_PENDING,
                    RX_FAULT_WAITING_FOR_STAGED_UNIT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            Esp32s31RxIngressObservation::Staged(_) => {
                let _ = self.state.compare_exchange(
                    RX_FAULT_WAITING_FOR_STAGED_UNIT,
                    RX_FAULT_COMPLETE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            _ => {}
        }
    }
}

#[unsafe(link_section = ".critical.data.open_radio_fault")]
pub static STATION_FAULT_CONTROL: StationFaultControl = StationFaultControl::new();

/// Failure returned by the HIL adapter without erasing the production error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultInjectingServicesError<E> {
    Inner(E),
    InjectedTxAfterPublication {
        fault: ArmedStationFault,
        source: E,
    },
    InjectionContractViolation {
        fault: ArmedStationFault,
        progress: WifiTxProgress,
    },
}

/// HIL decorator retaining the complete real production services graph.
pub struct FaultInjectingConnectedServices<'control, B> {
    inner: B,
    control: &'control StationFaultControl,
    active: Option<ArmedStationFault>,
}

impl<'control, B> FaultInjectingConnectedServices<'control, B> {
    pub const fn new(inner: B, control: &'control StationFaultControl) -> Self {
        Self {
            inner,
            control,
            active: None,
        }
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    pub const fn inner(&self) -> &B {
        &self.inner
    }
}

impl<
    'resources,
    M,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    for FaultInjectingConnectedServices<'_, B>
where
    M: RawMutex,
    B: ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    type Error = FaultInjectingServicesError<B::Error>;

    fn service_rx(
        &mut self,
    ) -> impl core::future::Future<Output = Result<WifiRxProgress, Self::Error>> + '_ {
        async move {
            let progress = self
                .inner
                .service_rx()
                .await
                .map_err(FaultInjectingServicesError::Inner)?;
            if let Some(fault) = self.control.take_recovered_rx_fault() {
                crate::console::publish_station_fault(
                    fault.request_id,
                    StationFaultEvidence::ConnectedRxOverCapacityRecovered {
                        classification: StationFaultClassification::RecoverableFrameDiscard,
                        descriptor_reloaded: true,
                        following_unit_staged: true,
                        same_ring_live: true,
                        service_result_ok: true,
                    },
                )
                .await;
            }
            Ok(progress)
        }
    }

    fn service_control<'a>(
        &'a mut self,
        context: WifiControlContext,
    ) -> impl core::future::Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            self.inner
                .service_control(context)
                .await
                .map_err(FaultInjectingServicesError::Inner)
        }
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl core::future::Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        self.inner.wait_control_ready()
    }

    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl core::future::Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = self
                .inner
                .start_tx(frame, network)
                .await
                .map_err(FaultInjectingServicesError::Inner)?;
            if progress == WifiTxProgress::Pending {
                self.active = self.control.take_after_tx_publication();
            }
            Ok(progress)
        }
    }

    fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        async move {
            if self.active.is_none() {
                self.inner.wait_tx_deadline().await;
            }
        }
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl core::future::Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let Some(fault) = self.active.take() else {
                return self
                    .inner
                    .service_tx(wake)
                    .await
                    .map_err(FaultInjectingServicesError::Inner);
            };
            let contradictory = WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT,
            };
            match self.inner.service_tx(contradictory).await {
                Err(source) => {
                    Err(FaultInjectingServicesError::InjectedTxAfterPublication { fault, source })
                }
                Ok(progress) => {
                    Err(FaultInjectingServicesError::InjectionContractViolation { fault, progress })
                }
            }
        }
    }
}
