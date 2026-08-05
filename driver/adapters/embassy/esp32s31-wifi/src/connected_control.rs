//! Embassy delivery adapter for ESP32-S31 connected-station control.
//!
//! Protocol state and finite transitions live in the chip STA crate.  This
//! module owns only the bounded event receiver, deadline wait and reorder
//! command sender required to schedule that core on Embassy.

use core::future::Future;

use embassy_futures::select::{Either, select};
use open_esp_radio_embassy_net::RawMutex;
pub use open_esp_radio_esp32s31_wifi_sta::{
    connected_control::{
        ConnectedControlContext, ConnectedControlError, ConnectedControlProgress,
        ConnectedControlReorder, ConnectedControlTx, ConnectedControlTxFailure,
        ConnectedControlTxKind, Esp32s31ConnectedControlCore, RxReorderCommand,
        RxReorderCommandError,
    },
    connected_control_hardware::ConnectedControlHardware,
};
use open_esp_radio_wifi_sta::{
    link_monitor::{StaBeaconLossConfig, StaBeaconMonitor},
    power_save::{StaDozePermit, StaPowerSavePlanner, StaPowerSavePolicy},
};

use crate::{
    connected_runner::{WifiControlContext, WifiControlProgress},
    connected_services::Esp32s31ControlService,
    control_mailbox::ConnectedControlReceiver,
    rx_reorder::{RxReorderCommandSender, try_send_rx_reorder_command},
};

/// Executor deadline capability kept outside the finite control core.
pub trait ConnectedControlTimer {
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTimer
    for open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::Esp32s31SingleMpduTx<
        '_,
        P,
        E,
        T,
        BUFFER_SIZE,
    >
where
    P: open_esp_radio_esp32s31_wifi_sta::ordinary_tx::WifiTxPowerProfile,
    E: open_esp_radio_esp32s31_wifi_sta::ordinary_tx::WifiTxEntropy,
    T: open_esp_radio_esp32s31_wifi_sta::ordinary_tx::WifiTxTimer,
{
    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::Esp32s31SingleMpduTx::wait_until_micros(
            self,
            deadline_micros,
        )
    }
}

/// Finite ownership released when one connected control epoch stops.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub discarded_events: u8,
    pub in_flight: Option<ConnectedControlTxKind>,
}

struct EmbassyReorderSink<'sender, 'resources, M: RawMutex> {
    sender: Option<&'sender RxReorderCommandSender<'resources, M>>,
}

impl<M: RawMutex> ConnectedControlReorder for EmbassyReorderSink<'_, '_, M> {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError> {
        let Some(sender) = self.sender else {
            return Ok(());
        };
        try_send_rx_reorder_command(sender, command)
    }
}

/// Unique Embassy event consumer around one executor-independent control core.
pub struct Esp32s31ConnectedControl<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    core: Esp32s31ConnectedControlCore,
    rx_reorder_commands: Option<RxReorderCommandSender<'resources, M>>,
}

impl<'resources, M: RawMutex, const CAPACITY: usize>
    Esp32s31ConnectedControl<'resources, M, CAPACITY>
{
    pub fn new(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: open_esp_radio_esp32s31_wifi_mac::tx_ampdu::StaTxBlockAckSessions,
    ) -> Self {
        Self {
            receiver,
            core: Esp32s31ConnectedControlCore::new(peer, he_enabled, tx_block_ack),
            rx_reorder_commands: None,
        }
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandSender<'resources, M>,
    ) -> Self {
        self.rx_reorder_commands = Some(commands);
        self
    }

    pub fn with_rx_block_ack_maximum_window(
        mut self,
        maximum_window: u16,
    ) -> Result<Self, open_esp_radio_esp32s31_wifi_mac::rx_ampdu::StaRxBlockAckSessionsError> {
        self.core.set_rx_block_ack_maximum_window(maximum_window)?;
        Ok(self)
    }

    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.core.enable_beacon_loss(config);
    }

    pub fn enable_power_save(&mut self, policy: StaPowerSavePolicy) {
        self.core.enable_power_save(policy);
    }

    pub fn queue_initial_tx_block_ack(&mut self) {
        self.core.queue_initial_tx_block_ack();
    }

    pub const fn rx_block_ack(
        &self,
    ) -> &open_esp_radio_esp32s31_wifi_mac::rx_ampdu::StaRxBlockAckSessions {
        self.core.rx_block_ack()
    }

    pub const fn tx_block_ack(
        &self,
    ) -> &open_esp_radio_esp32s31_wifi_mac::tx_ampdu::StaTxBlockAckSessions {
        self.core.tx_block_ack()
    }

    pub const fn last_event(
        &self,
    ) -> Option<open_esp_radio_esp32s31_wifi_mac::connected_rx::ConnectedRxControlEvent> {
        self.core.last_event()
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.core.last_tx_failure()
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.core.last_expired_tid()
    }

    pub const fn beacon_monitor(&self) -> Option<&StaBeaconMonitor> {
        self.core.beacon_monitor()
    }

    pub const fn beacon_lost(&self) -> bool {
        self.core.beacon_lost()
    }

    pub const fn power_save(&self) -> Option<&StaPowerSavePlanner> {
        self.core.power_save()
    }

    pub fn take_doze_permit(&mut self) -> Option<StaDozePermit> {
        self.core.take_doze_permit()
    }

    pub fn dropped_events(&self) -> u32 {
        self.receiver.dropped()
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<ConnectedControlShutdown, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let shutdown = self.core.shutdown(hardware, tx)?;
        let mut discarded_events = 0_u8;
        while self.receiver.try_receive().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: shutdown.rx_block_ack_agreements,
            tx_block_ack_sessions: shutdown.tx_block_ack_sessions,
            discarded_events,
            in_flight: shutdown.in_flight,
        })
    }

    fn has_immediate_work(&self) -> bool {
        self.core.has_immediate_work(self.receiver.len() != 0)
    }

    /// Wait without consuming the event that made control work ready.
    pub fn wait_ready<'a, X>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a
    where
        X: ConnectedControlTx + ConnectedControlTimer + 'a,
    {
        async move {
            if self.has_immediate_work() {
                return;
            }
            if let Some(deadline) = self.core.next_alarm_deadline() {
                match select(self.receiver.ready(), tx.wait_until_micros(deadline)).await {
                    Either::First(()) | Either::Second(()) => {}
                }
            } else {
                self.receiver.ready().await;
            }
        }
    }

    pub fn service<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
    ) -> impl Future<Output = Result<WifiControlProgress, ConnectedControlError>> + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        self.service_with_context(hardware, tx, WifiControlContext::IDLE)
    }

    pub fn service_with_context<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, ConnectedControlError>> + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        async move {
            // A completion owns priority over newly queued RX control.  Do not
            // dequeue an event until the core can consume it in this step.
            let event = if self.core.tx_in_flight() {
                None
            } else {
                self.receiver.try_receive()
            };
            let control_event_pending = self.receiver.len() != 0;
            let mut reorder = EmbassyReorderSink {
                sender: self.rx_reorder_commands.as_ref(),
            };
            self.core.service_step(
                hardware,
                tx,
                &mut reorder,
                event,
                control_event_pending,
                context,
            )
        }
    }
}

impl<'resources, M, H, X, const CAPACITY: usize> Esp32s31ControlService<H, X>
    for Esp32s31ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx + ConnectedControlTimer,
{
    type Error = ConnectedControlError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a {
        Esp32s31ConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        Esp32s31ConnectedControl::wait_ready(self, tx)
    }
}

#[cfg(test)]
#[path = "connected_control_tests.rs"]
mod tests;
