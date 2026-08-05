//! HIL-only fault adapter around the production connected radio services.
//!
//! The adapter never fabricates a station/lifecycle result. It first lets the
//! real services acquire a network lease and publish a MAC descriptor. Only
//! then does it replace the next TX wake with an impossible simultaneous
//! completion/timeout image. The production ordinary/A-MPDU transaction must
//! classify that edge as reset-required and quarantine its real owner.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use open_esp_radio::esp32s31::wifi::lmac::irq::{MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT};
use open_esp_radio::integration::esp32s31::wifi_embassy::connected_runner::{
    ConnectedRunnerServices, WifiControlContext, WifiControlProgress, WifiRxProgress,
    WifiTxProgress, WifiTxWake,
};
use open_esp_radio::integration::network::embassy_net::{
    PinnedTxConsumer, PinnedTxFrame, RawMutex,
};
use open_esp_radio_hil_protocol::StationFaultInjection;

const FAULT_IDLE: u8 = 0;
const FAULT_ARMED: u8 = 1;
const FAULT_ACTIVE: u8 = 2;

/// Correlation retained from the host request to the terminal owner frontier.
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
        // There is currently one wire-visible injection point. Keep the
        // explicit match so extending the protocol cannot silently select the
        // wrong runtime behavior.
        match injection {
            StationFaultInjection::ConnectedTxAfterPublication => {}
        }
        if self
            .state
            .compare_exchange(
                FAULT_IDLE,
                FAULT_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        self.request_id.store(request_id, Ordering::Relaxed);
        self.state.store(FAULT_ARMED, Ordering::Release);
        true
    }

    fn take_after_tx_publication(&self) -> Option<ArmedStationFault> {
        self.state
            .compare_exchange(
                FAULT_ARMED,
                FAULT_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        Some(ArmedStationFault {
            request_id: self.request_id.load(Ordering::Relaxed),
            injection: StationFaultInjection::ConnectedTxAfterPublication,
        })
    }
}

impl Default for StationFaultControl {
    fn default() -> Self {
        Self::new()
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
            self.inner
                .service_rx()
                .await
                .map_err(FaultInjectingServicesError::Inner)
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
