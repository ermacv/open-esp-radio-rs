#![expect(
    clippy::type_complexity,
    reason = "the stopped value returns every affine IRQ/RX/TX/protocol owner"
)]

//! Bounded standalone ESP-NOW runtime and stop/restart owner transition.

use core::{future::Future, pin::pin};

use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi::{
    esp_now::Esp32s31EspNowTxConfig,
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_mac::{
    init::{
        MAC_COLD_RX_INTERRUPT_MASK, StaEspNowRxPolicyHardware,
        configure_standalone_esp_now_receive_policy,
    },
    irq::MacInterruptRoute,
    tx::TxHardware,
};
use open_esp_radio_esp32s31_wifi_sta::{
    control_tx::Esp32s31ControlTx,
    single_mpdu_tx::{SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
};
use open_esp_radio_ieee80211::{channel::WifiChannel, station::StaSequenceCounter};
use open_esp_radio_wifi_softmac::{
    EspNowProtocol, WifiStandaloneEspNowPlan, interface::BoundVirtualInterface,
};

use crate::{
    datapath::{
        irq::{
            Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
            Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
        },
        rx::frontier::Esp32s31RxFrontierPhase,
    },
    roles::{
        esp_now::rx::{Esp32s31StandaloneEspNowReceive, Esp32s31StandaloneEspNowRxProgress},
        station::esp_now_tx::{
            EspNowQueuedTx, EspNowTxCancelReason, EspNowTxMailboxInvariantError,
            EspNowTxMailboxOwner, EspNowTxMailboxShutdown, EspNowTxRuntimeFailure,
            EspNowTxTerminal,
        },
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StandaloneEspNowBindingError {
    StationBinding {
        topology: BoundVirtualInterface,
        protocol: BoundVirtualInterface,
    },
    ChannelBinding {
        protocol: WifiChannel,
        active: WifiChannel,
    },
}

/// Fully checked identity retained while the integration excludes retuning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StandaloneEspNowBinding {
    station: BoundVirtualInterface,
    channel: WifiChannel,
    tx: Esp32s31EspNowTxConfig,
}

impl Esp32s31StandaloneEspNowBinding {
    pub fn new<const PEERS: usize>(
        plan: WifiStandaloneEspNowPlan,
        protocol: &EspNowProtocol<PEERS>,
        active_channel: WifiChannel,
        tx: Esp32s31EspNowTxConfig,
    ) -> Result<Self, Esp32s31StandaloneEspNowBindingError> {
        let station = plan.station();
        let configured = protocol.config();
        if configured.station() != station {
            return Err(Esp32s31StandaloneEspNowBindingError::StationBinding {
                topology: station,
                protocol: configured.station(),
            });
        }
        if configured.home_channel() != active_channel {
            return Err(Esp32s31StandaloneEspNowBindingError::ChannelBinding {
                protocol: configured.home_channel(),
                active: active_channel,
            });
        }
        Ok(Self {
            station,
            channel: active_channel,
            tx,
        })
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel
    }

    pub const fn tx_config(self) -> Esp32s31EspNowTxConfig {
        self.tx
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31StandaloneEspNowRunReport {
    pub receive_wakes: u32,
    pub receive: Esp32s31StandaloneEspNowRxProgress,
    pub tx_started: u32,
    pub tx_completed: u32,
    pub tx_rejected: u32,
    pub duplicate_history_cleared: usize,
    pub mailbox: Option<EspNowTxMailboxShutdown>,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StandaloneEspNowStopError<InterruptError, ReceiveError> {
    NotRunning,
    Tx(SingleMpduTxError),
    MissingOrdinaryTxOutcome,
    Mailbox(EspNowTxMailboxInvariantError),
    Interrupt(Esp32s31MacInterruptEpochQuiesceError<InterruptError>),
    Receive(ReceiveError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StandaloneEspNowRunError<InterruptError, ReceiveError> {
    AlreadyRunning,
    AlreadyStopped,
    Faulted,
    ReceiveStationBinding {
        expected: BoundVirtualInterface,
        actual: BoundVirtualInterface,
    },
    ReceiveChannelBinding {
        expected: WifiChannel,
        actual: WifiChannel,
    },
    ReceivePeerSnapshotMismatch,
    ReceiveNotPrepared(Esp32s31RxFrontierPhase),
    TransmitNotIdle,
    ReceiveStart(ReceiveError),
    Activate(Esp32s31MacInterruptEpochActivateError<InterruptError>),
    ActivateReceiveStop {
        activation: Esp32s31MacInterruptEpochActivateError<InterruptError>,
        receive: ReceiveError,
    },
    ReceiveService(ReceiveError),
    Tx(SingleMpduTxError),
    MissingOrdinaryTxOutcome,
    Mailbox(EspNowTxMailboxInvariantError),
    Stop(Esp32s31StandaloneEspNowStopError<InterruptError, ReceiveError>),
}

pub struct Esp32s31StandaloneEspNowRunFailure<InterruptError, ReceiveError> {
    pub error: Esp32s31StandaloneEspNowRunError<InterruptError, ReceiveError>,
    pub report: Esp32s31StandaloneEspNowRunReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServicePhase {
    Prepared,
    Running,
    Stopped,
    Faulted,
}

enum IdleWake {
    Stop,
    Receive,
    Transmit,
}

/// Complete standalone radio owner.
///
/// `RX` is a normal station RX frontier. The sole application TX endpoint is
/// the same bounded mailbox used by connected ESP-NOW; the sole ordinary TX
/// descriptor is the pre-connected owner and therefore carries no WPA2 key.
pub struct Esp32s31StandaloneEspNowService<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    hardware: Option<H>,
    receive: Option<RX>,
    sink: Option<S>,
    interrupts: Option<Esp32s31MacInterruptEpoch<'runtime, R, M>>,
    platform: Option<R::Platform>,
    tx: Option<Esp32s31ControlTx<'slot, P, E, T, TX_BUFFER_SIZE>>,
    mailbox: Option<EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>>,
    protocol: Option<EspNowProtocol<PEERS>>,
    sequence: StaSequenceCounter,
    binding: Esp32s31StandaloneEspNowBinding,
    active: Option<EspNowQueuedTx>,
    prefer_receive: bool,
    phase: ServicePhase,
    report: Esp32s31StandaloneEspNowRunReport,
}

impl<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    #[allow(clippy::too_many_arguments)]
    pub fn from_binding(
        hardware: H,
        receive: RX,
        sink: S,
        interrupts: Esp32s31MacInterruptEpoch<'runtime, R, M>,
        platform: R::Platform,
        tx: Esp32s31ControlTx<'slot, P, E, T, TX_BUFFER_SIZE>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        sequence: StaSequenceCounter,
        binding: Esp32s31StandaloneEspNowBinding,
    ) -> Self {
        debug_assert_eq!(protocol.config().station(), binding.station);
        debug_assert_eq!(protocol.config().home_channel(), binding.channel);
        Self {
            hardware: Some(hardware),
            receive: Some(receive),
            sink: Some(sink),
            interrupts: Some(interrupts),
            platform: Some(platform),
            tx: Some(tx),
            mailbox: Some(mailbox),
            protocol: Some(protocol),
            sequence,
            binding,
            active: None,
            prefer_receive: true,
            phase: ServicePhase::Prepared,
            report: Esp32s31StandaloneEspNowRunReport::default(),
        }
    }

    pub const fn binding(&self) -> Esp32s31StandaloneEspNowBinding {
        self.binding
    }

    pub const fn tx_epoch(&self) -> Option<u32> {
        match &self.mailbox {
            Some(mailbox) => Some(mailbox.epoch()),
            None => None,
        }
    }

    pub const fn report(&self) -> Esp32s31StandaloneEspNowRunReport {
        self.report
    }
}

impl<
    'slot,
    'resources,
    'runtime,
    M,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    M: RawMutex,
    H: StaEspNowRxPolicyHardware + TxHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    RX: Esp32s31StandaloneEspNowReceive<H, S, PEERS>,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Run until `stop` resolves, then close all application admission and
    /// return with IRQ, RX DMA and ordinary TX quiescent.
    pub async fn run_until_stopped<F>(
        &mut self,
        stop: F,
    ) -> Result<
        Esp32s31StandaloneEspNowRunReport,
        Esp32s31StandaloneEspNowRunFailure<R::Error, RX::Error>,
    >
    where
        F: Future<Output = ()>,
    {
        match self.phase {
            ServicePhase::Prepared => {}
            ServicePhase::Running => {
                return Err(self.failure(Esp32s31StandaloneEspNowRunError::AlreadyRunning));
            }
            ServicePhase::Stopped => {
                return Err(self.failure(Esp32s31StandaloneEspNowRunError::AlreadyStopped));
            }
            ServicePhase::Faulted => {
                return Err(self.failure(Esp32s31StandaloneEspNowRunError::Faulted));
            }
        }
        let receive_phase = self.receive().phase();
        let receive_station = self.receive().station();
        if receive_station != self.binding.station {
            return Err(
                self.failure(Esp32s31StandaloneEspNowRunError::ReceiveStationBinding {
                    expected: self.binding.station,
                    actual: receive_station,
                }),
            );
        }
        let receive_channel = self.receive().home_channel();
        if receive_channel != self.binding.channel {
            return Err(
                self.failure(Esp32s31StandaloneEspNowRunError::ReceiveChannelBinding {
                    expected: self.binding.channel,
                    actual: receive_channel,
                }),
            );
        }
        if !self.receive().peer_snapshot_matches(
            self.protocol
                .as_ref()
                .expect("ESP-NOW protocol owner exists"),
        ) {
            return Err(self.failure(Esp32s31StandaloneEspNowRunError::ReceivePeerSnapshotMismatch));
        }
        if receive_phase != Esp32s31RxFrontierPhase::Prepared {
            return Err(
                self.failure(Esp32s31StandaloneEspNowRunError::ReceiveNotPrepared(
                    receive_phase,
                )),
            );
        }
        if self.tx().active() {
            return Err(self.failure(Esp32s31StandaloneEspNowRunError::TransmitNotIdle));
        }

        configure_standalone_esp_now_receive_policy(self.hardware_mut());
        if let Err(error) = self.start_receive() {
            return Err(self.failure(Esp32s31StandaloneEspNowRunError::ReceiveStart(error)));
        }
        let activation = {
            let platform = self
                .platform
                .as_ref()
                .expect("ESP-NOW platform owner exists");
            self.interrupts
                .as_mut()
                .expect("ESP-NOW interrupt owner exists")
                .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
        };
        if let Err(activation) = activation {
            return match self.stop_receive().await {
                Ok(()) => Err(self.failure(Esp32s31StandaloneEspNowRunError::Activate(activation))),
                Err(receive) => {
                    self.phase = ServicePhase::Faulted;
                    Err(
                        self.failure(Esp32s31StandaloneEspNowRunError::ActivateReceiveStop {
                            activation,
                            receive,
                        }),
                    )
                }
            };
        }
        self.phase = ServicePhase::Running;
        self.interrupts().mac_runtime().notify_rx_handoff();

        let mut stop = pin!(stop);
        loop {
            if self.tx().active() {
                let mac = self.interrupts().mac_runtime();
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                match select4(
                    stop.as_mut(),
                    mac.wait_tx(),
                    tx.wait_deadline(),
                    mac.wait_rx(),
                )
                .await
                {
                    Either4::First(()) => break,
                    Either4::Second(events) => {
                        match self
                            .service_transmit(WifiTxWake::Interrupt { events })
                            .await
                        {
                            Ok(_) => {}
                            Err(Esp32s31StandaloneEspNowRunError::Tx(error)) => {
                                return self.fail_transmit(error);
                            }
                            Err(error) => return self.fail_and_stop(error).await,
                        }
                    }
                    Either4::Third(()) => match self.service_transmit(WifiTxWake::Deadline).await {
                        Ok(_) => {}
                        Err(Esp32s31StandaloneEspNowRunError::Tx(error)) => {
                            return self.fail_transmit(error);
                        }
                        Err(error) => return self.fail_and_stop(error).await,
                    },
                    Either4::Fourth(()) => {
                        if let Err(error) = self.service_receive().await {
                            return self
                                .fail_and_stop(Esp32s31StandaloneEspNowRunError::ReceiveService(
                                    error,
                                ))
                                .await;
                        }
                    }
                }
            } else {
                let mac = self.interrupts().mac_runtime();
                let mailbox = self.mailbox();
                let wake = if self.prefer_receive {
                    match select3(stop.as_mut(), mac.wait_rx(), mailbox.ready()).await {
                        Either3::First(()) => IdleWake::Stop,
                        Either3::Second(()) => IdleWake::Receive,
                        Either3::Third(()) => IdleWake::Transmit,
                    }
                } else {
                    match select3(stop.as_mut(), mailbox.ready(), mac.wait_rx()).await {
                        Either3::First(()) => IdleWake::Stop,
                        Either3::Second(()) => IdleWake::Transmit,
                        Either3::Third(()) => IdleWake::Receive,
                    }
                };
                match wake {
                    IdleWake::Stop => break,
                    IdleWake::Receive => {
                        self.prefer_receive = false;
                        if let Err(error) = self.service_receive().await {
                            return self
                                .fail_and_stop(Esp32s31StandaloneEspNowRunError::ReceiveService(
                                    error,
                                ))
                                .await;
                        }
                    }
                    IdleWake::Transmit => {
                        self.prefer_receive = true;
                        if let Err(error) = self.start_next_transmit() {
                            return self.fail_and_stop(error).await;
                        }
                    }
                }
            }
        }

        match self
            .stop_running(EspNowTxCancelReason::StationStopped)
            .await
        {
            Ok(()) => Ok(self.report),
            Err(error) => Err(self.failure(Esp32s31StandaloneEspNowRunError::Stop(error))),
        }
    }

    /// Explicit cancellation recovery for a dropped run future.
    pub async fn stop(
        &mut self,
    ) -> Result<
        Esp32s31StandaloneEspNowRunReport,
        Esp32s31StandaloneEspNowStopError<R::Error, RX::Error>,
    > {
        match self.phase {
            ServicePhase::Prepared => {
                self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
                self.finish_mailbox(EspNowTxCancelReason::StationStopped)
                    .await?;
                self.report.duplicate_history_cleared =
                    self.receive_mut().reset_duplicate_history();
                self.phase = ServicePhase::Stopped;
                Ok(self.report)
            }
            ServicePhase::Running => {
                self.stop_running(EspNowTxCancelReason::StationStopped)
                    .await?;
                Ok(self.report)
            }
            ServicePhase::Stopped | ServicePhase::Faulted => {
                Err(Esp32s31StandaloneEspNowStopError::NotRunning)
            }
        }
    }

    fn start_receive(&mut self) -> Result<(), RX::Error> {
        let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
        let hardware = self
            .hardware
            .as_mut()
            .expect("ESP-NOW hardware owner exists");
        receive.start(hardware)
    }

    async fn service_receive(&mut self) -> Result<(), RX::Error> {
        self.report.receive_wakes = self.report.receive_wakes.saturating_add(1);
        let progress = {
            let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            let sink = self.sink.as_mut().expect("ESP-NOW RX sink exists");
            receive.service(hardware, sink)?
        };
        record_receive(&mut self.report.receive, progress);
        if progress.service_probe_pending {
            yield_now().await;
            self.interrupts().mac_runtime().notify_rx_handoff();
        }
        Ok(())
    }

    fn start_next_transmit(
        &mut self,
    ) -> Result<(), Esp32s31StandaloneEspNowRunError<R::Error, RX::Error>> {
        let Some(queued) = self.mailbox().try_take() else {
            return Ok(());
        };
        if queued.ticket.epoch() != self.mailbox().epoch() {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StaleEpoch),
                )
                .map_err(Esp32s31StandaloneEspNowRunError::Mailbox)?;
            return Ok(());
        }

        // No ordinary transaction is live, so any coalesced TX signal belongs
        // to an earlier role/transaction and must not complete this request.
        let _ = self.interrupts().mac_runtime().try_take_tx();
        let result = {
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
            let protocol = self
                .protocol
                .as_ref()
                .expect("ESP-NOW protocol owner exists");
            tx.start_esp_now_v1_plaintext(
                hardware,
                protocol,
                &mut self.sequence,
                queued.request.peer(),
                queued.request.random_value(),
                queued.request.payload(),
                self.binding.channel,
                self.binding.station,
                self.binding.tx,
            )
        };
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active = Some(queued);
                self.report.tx_started = self.report.tx_started.saturating_add(1);
            }
            Ok(WifiTxProgress::Complete) => {
                self.active = Some(queued);
                self.report.tx_started = self.report.tx_started.saturating_add(1);
                self.finish_active()?;
            }
            Err(error) => {
                self.report.tx_rejected = self.report.tx_rejected.saturating_add(1);
                self.mailbox()
                    .publish(queued, EspNowTxTerminal::Rejected(error))
                    .map_err(Esp32s31StandaloneEspNowRunError::Mailbox)?;
            }
        }
        Ok(())
    }

    async fn service_transmit(
        &mut self,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31StandaloneEspNowRunError<R::Error, RX::Error>> {
        let progress = {
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
            tx.service(hardware, wake)
                .await
                .map_err(Esp32s31StandaloneEspNowRunError::Tx)?
        };
        if progress == WifiTxProgress::Complete {
            self.finish_active()?;
        }
        Ok(progress)
    }

    fn finish_active(
        &mut self,
    ) -> Result<(), Esp32s31StandaloneEspNowRunError<R::Error, RX::Error>> {
        let Some(queued) = self.active.take() else {
            return Ok(());
        };
        let Some(outcome) = self.tx_mut().take_last_outcome() else {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::RuntimeFailure(
                        EspNowTxRuntimeFailure::MissingOrdinaryTxOutcome,
                    ),
                )
                .map_err(Esp32s31StandaloneEspNowRunError::Mailbox)?;
            return Err(Esp32s31StandaloneEspNowRunError::MissingOrdinaryTxOutcome);
        };
        self.mailbox()
            .publish(queued, EspNowTxTerminal::Completed(outcome))
            .map_err(Esp32s31StandaloneEspNowRunError::Mailbox)?;
        self.report.tx_completed = self.report.tx_completed.saturating_add(1);
        Ok(())
    }

    fn fail_transmit(
        &mut self,
        error: SingleMpduTxError,
    ) -> Result<
        Esp32s31StandaloneEspNowRunReport,
        Esp32s31StandaloneEspNowRunFailure<R::Error, RX::Error>,
    > {
        if let Some(queued) = self.active.take() {
            let _ = self.mailbox().publish(
                queued,
                EspNowTxTerminal::RuntimeFailure(EspNowTxRuntimeFailure::TxLifecycle(error)),
            );
        }
        let _ = self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown);
        self.phase = ServicePhase::Faulted;
        Err(self.failure(Esp32s31StandaloneEspNowRunError::Tx(error)))
    }

    async fn fail_and_stop(
        &mut self,
        error: Esp32s31StandaloneEspNowRunError<R::Error, RX::Error>,
    ) -> Result<
        Esp32s31StandaloneEspNowRunReport,
        Esp32s31StandaloneEspNowRunFailure<R::Error, RX::Error>,
    > {
        match self.stop_running(EspNowTxCancelReason::OwnerShutdown).await {
            Ok(()) => Err(self.failure(error)),
            Err(stop) => Err(self.failure(Esp32s31StandaloneEspNowRunError::Stop(stop))),
        }
    }

    async fn stop_running(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<(), Esp32s31StandaloneEspNowStopError<R::Error, RX::Error>> {
        self.close_and_cancel(reason)?;
        while self.tx().active() {
            let mac = self.interrupts().mac_runtime();
            let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
            let wake = match select(mac.wait_tx(), tx.wait_deadline()).await {
                Either::First(events) => WifiTxWake::Interrupt { events },
                Either::Second(()) => WifiTxWake::Deadline,
            };
            let progress = {
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                tx.service(hardware, wake)
                    .await
                    .map_err(Esp32s31StandaloneEspNowStopError::Tx)?
            };
            if progress == WifiTxProgress::Complete {
                self.finish_active().map_err(|error| match error {
                    Esp32s31StandaloneEspNowRunError::Mailbox(error) => {
                        Esp32s31StandaloneEspNowStopError::Mailbox(error)
                    }
                    Esp32s31StandaloneEspNowRunError::MissingOrdinaryTxOutcome => {
                        Esp32s31StandaloneEspNowStopError::MissingOrdinaryTxOutcome
                    }
                    _ => Esp32s31StandaloneEspNowStopError::MissingOrdinaryTxOutcome,
                })?;
            }
        }
        self.finish_mailbox(reason).await?;

        if self.interrupts().is_active() {
            let platform = self
                .platform
                .as_ref()
                .expect("ESP-NOW platform owner exists");
            self.report.interrupt_drain = self
                .interrupts
                .as_mut()
                .expect("ESP-NOW interrupt owner exists")
                .quiesce(platform)
                .map_err(Esp32s31StandaloneEspNowStopError::Interrupt)?;
        }
        self.stop_receive()
            .await
            .map_err(Esp32s31StandaloneEspNowStopError::Receive)?;
        self.report.duplicate_history_cleared = self.receive_mut().reset_duplicate_history();
        self.phase = ServicePhase::Stopped;
        Ok(())
    }

    fn close_and_cancel(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, Esp32s31StandaloneEspNowStopError<R::Error, RX::Error>> {
        let mailbox = self
            .mailbox
            .as_mut()
            .ok_or(Esp32s31StandaloneEspNowStopError::NotRunning)?;
        mailbox.close();
        mailbox
            .cancel_pending(reason)
            .map_err(Esp32s31StandaloneEspNowStopError::Mailbox)
    }

    async fn finish_mailbox(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<(), Esp32s31StandaloneEspNowStopError<R::Error, RX::Error>> {
        while self.mailbox().publishers_in_flight() != 0 {
            yield_now().await;
        }
        self.close_and_cancel(reason)?;
        let mailbox = self
            .mailbox
            .take()
            .ok_or(Esp32s31StandaloneEspNowStopError::NotRunning)?;
        self.report.mailbox = Some(
            mailbox
                .shutdown(reason)
                .map_err(Esp32s31StandaloneEspNowStopError::Mailbox)?,
        );
        Ok(())
    }

    async fn stop_receive(&mut self) -> Result<(), RX::Error> {
        while self.receive().phase() == Esp32s31RxFrontierPhase::Live {
            let stopped = {
                let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                receive.stop(hardware)?
            };
            if !stopped {
                yield_now().await;
            }
        }
        Ok(())
    }

    fn failure(
        &self,
        error: Esp32s31StandaloneEspNowRunError<R::Error, RX::Error>,
    ) -> Esp32s31StandaloneEspNowRunFailure<R::Error, RX::Error> {
        Esp32s31StandaloneEspNowRunFailure {
            error,
            report: self.report,
        }
    }

    fn hardware_mut(&mut self) -> &mut H {
        self.hardware
            .as_mut()
            .expect("ESP-NOW hardware owner exists")
    }

    fn receive(&self) -> &RX {
        self.receive.as_ref().expect("ESP-NOW RX owner exists")
    }

    fn receive_mut(&mut self) -> &mut RX {
        self.receive.as_mut().expect("ESP-NOW RX owner exists")
    }

    fn interrupts(&self) -> &Esp32s31MacInterruptEpoch<'runtime, R, M> {
        self.interrupts
            .as_ref()
            .expect("ESP-NOW interrupt owner exists")
    }

    fn tx(&self) -> &Esp32s31ControlTx<'slot, P, E, T, TX_BUFFER_SIZE> {
        self.tx.as_ref().expect("ESP-NOW TX owner exists")
    }

    fn tx_mut(&mut self) -> &mut Esp32s31ControlTx<'slot, P, E, T, TX_BUFFER_SIZE> {
        self.tx.as_mut().expect("ESP-NOW TX owner exists")
    }

    fn mailbox(&self) -> &EspNowTxMailboxOwner<'resources, M, TX_CAPACITY> {
        self.mailbox.as_ref().expect("ESP-NOW mailbox owner exists")
    }
}

/// Every owner returned after a successful stopped edge. A new mailbox epoch
/// and a re-prepared halted RX frontier can construct the next runtime epoch.
pub struct Esp32s31StandaloneEspNowStopped<
    'slot,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    pub hardware: H,
    pub receive: RX,
    pub sink: S,
    pub interrupts: Esp32s31MacInterruptEpoch<'runtime, R, M>,
    pub platform: R::Platform,
    pub tx: Esp32s31ControlTx<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub protocol: EspNowProtocol<PEERS>,
    pub sequence: StaSequenceCounter,
    pub binding: Esp32s31StandaloneEspNowBinding,
    pub report: Esp32s31StandaloneEspNowRunReport,
}

impl<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    #[allow(clippy::result_large_err)]
    pub fn try_into_stopped(
        mut self,
    ) -> Result<
        Esp32s31StandaloneEspNowStopped<
            'slot,
            'runtime,
            M,
            H,
            R,
            RX,
            S,
            P,
            E,
            T,
            PEERS,
            TX_BUFFER_SIZE,
        >,
        Self,
    > {
        if self.phase != ServicePhase::Stopped
            || self.active.is_some()
            || self.mailbox.is_some()
            || self
                .interrupts
                .as_ref()
                .is_some_and(Esp32s31MacInterruptEpoch::is_active)
        {
            return Err(self);
        }
        Ok(Esp32s31StandaloneEspNowStopped {
            hardware: self.hardware.take().expect("checked hardware owner"),
            receive: self.receive.take().expect("checked RX owner"),
            sink: self.sink.take().expect("checked RX sink"),
            interrupts: self.interrupts.take().expect("checked IRQ owner"),
            platform: self.platform.take().expect("checked platform owner"),
            tx: self.tx.take().expect("checked TX owner"),
            protocol: self.protocol.take().expect("checked protocol owner"),
            sequence: self.sequence,
            binding: self.binding,
            report: self.report,
        })
    }

    fn retain_active_owners_for_reset(&mut self) {
        core::mem::forget(self.hardware.take());
        core::mem::forget(self.receive.take());
        core::mem::forget(self.sink.take());
        core::mem::forget(self.interrupts.take());
        core::mem::forget(self.platform.take());
        core::mem::forget(self.tx.take());
    }
}

impl<
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> Drop
    for Esp32s31StandaloneEspNowService<
        '_,
        '_,
        '_,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    fn drop(&mut self) {
        if matches!(self.phase, ServicePhase::Running | ServicePhase::Faulted) {
            self.retain_active_owners_for_reset();
        }
    }
}

fn record_receive(
    total: &mut Esp32s31StandaloneEspNowRxProgress,
    progress: Esp32s31StandaloneEspNowRxProgress,
) {
    total.completed_descriptors = total
        .completed_descriptors
        .saturating_add(progress.completed_descriptors);
    total.received = total.received.saturating_add(progress.received);
    total.duplicates = total.duplicates.saturating_add(progress.duplicates);
    total.ignored = total.ignored.saturating_add(progress.ignored);
    total.rejected = total.rejected.saturating_add(progress.rejected);
    total.recycled_descriptors = total
        .recycled_descriptors
        .saturating_add(progress.recycled_descriptors);
    total.reload_pending = progress.reload_pending;
    total.service_probe_pending = progress.service_probe_pending;
}
