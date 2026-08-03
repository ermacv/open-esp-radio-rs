//! Typed control-plane handoff for receive BlockAck reordering.
//!
//! ADDBA/DELBA processing owns protocol and PAC state in the connected
//! control task. Staged frame leases belong exclusively to the RX protocol
//! task. This bounded mailbox carries only the semantic agreement edge
//! between those owners; it never carries a frame pointer or C context.

use embassy_sync::channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError};
use open_esp_radio_embassy_net::RawMutex;

/// One command per possible RX agreement plus replacement/teardown slack.
///
/// The command path is not a packet queue. Capacity only has to cover finite
/// control-plane progress while the RX protocol task is scheduled elsewhere.
pub const RX_REORDER_COMMAND_CAPACITY: usize = 16;

/// Credits reserved for frames arriving while a retained reorder run is
/// released into the upper network owner.
///
/// A 40-slot ESP32-S31 HIL profile with an advertised 16-frame window reached
/// 15 retained leases while the remaining 25 slots were simultaneously owned
/// by DMA/protocol handoff. One complete 32-descriptor frontier is therefore
/// the minimum qualified overlap for a sustained receive profile.
pub const RX_REORDER_OVERLAP_SLOT_RESERVE: usize = 32;

/// Vendor receive reorder age before the first buffered run crosses a gap.
///
/// Complete `libnet80211.a[ieee80211_ht.o]::ieee80211_ampdu_reorder` calls
/// `ieee80211_ampdu_start_age_timer(0x493e0)` exactly when the first frame is
/// retained. The call is routed through the microsecond OSI timer-arm slot, so
/// the source-owned Embassy replacement keeps the same 300,000-us edge.
pub const RX_REORDER_GAP_TIMEOUT_MICROS: u64 = 300_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommand {
    /// Install or replace the receive reorder state for one QoS TID.
    Start {
        tid: u8,
        starting_sequence: u16,
        window: u16,
    },
    /// Flush and remove the receive reorder state for one QoS TID.
    Stop { tid: u8 },
    /// Flush every agreement when the connected ownership epoch ends.
    StopAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommandError {
    Full(RxReorderCommand),
}

pub type RxReorderCommandSender<'resources, M> =
    Sender<'resources, M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>;
pub type RxReorderCommandReceiver<'resources, M> =
    Receiver<'resources, M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>;

/// Static storage shared by the connected control and RX protocol tasks.
pub struct RxReorderCommandResources<M: RawMutex> {
    commands: Channel<M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>,
}

impl<M: RawMutex> RxReorderCommandResources<M> {
    pub const fn new() -> Self {
        Self {
            commands: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        RxReorderCommandSender<'_, M>,
        RxReorderCommandReceiver<'_, M>,
    ) {
        (self.commands.sender(), self.commands.receiver())
    }
}

impl<M: RawMutex> Default for RxReorderCommandResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn try_send_rx_reorder_command<M: RawMutex>(
    sender: &RxReorderCommandSender<'_, M>,
    command: RxReorderCommand,
) -> Result<(), RxReorderCommandError> {
    sender.try_send(command).map_err(|error| match error {
        TrySendError::Full(command) => RxReorderCommandError::Full(command),
    })
}

pub fn try_receive_rx_reorder_command<M: RawMutex>(
    receiver: &RxReorderCommandReceiver<'_, M>,
) -> Option<RxReorderCommand> {
    match receiver.try_receive() {
        Ok(command) => Some(command),
        Err(TryReceiveError::Empty) => None,
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_embassy_net::NoopRawMutex;

    use super::*;

    #[test]
    fn mailbox_preserves_owned_agreement_edges_in_order() {
        let resources = RxReorderCommandResources::<NoopRawMutex>::new();
        let (sender, receiver) = resources.split();
        let start = RxReorderCommand::Start {
            tid: 3,
            starting_sequence: 0x0ffe,
            window: 32,
        };
        try_send_rx_reorder_command(&sender, start).unwrap();
        try_send_rx_reorder_command(&sender, RxReorderCommand::Stop { tid: 3 }).unwrap();
        try_send_rx_reorder_command(&sender, RxReorderCommand::StopAll).unwrap();

        assert_eq!(try_receive_rx_reorder_command(&receiver), Some(start));
        assert_eq!(
            try_receive_rx_reorder_command(&receiver),
            Some(RxReorderCommand::Stop { tid: 3 })
        );
        assert_eq!(
            try_receive_rx_reorder_command(&receiver),
            Some(RxReorderCommand::StopAll)
        );
        assert_eq!(try_receive_rx_reorder_command(&receiver), None);
    }

    #[test]
    fn full_mailbox_returns_the_unpublished_command() {
        let resources = RxReorderCommandResources::<NoopRawMutex>::new();
        let (sender, _receiver) = resources.split();
        for tid in 0..RX_REORDER_COMMAND_CAPACITY {
            try_send_rx_reorder_command(&sender, RxReorderCommand::Stop { tid: tid as u8 })
                .unwrap();
        }
        assert_eq!(
            try_send_rx_reorder_command(&sender, RxReorderCommand::StopAll),
            Err(RxReorderCommandError::Full(RxReorderCommand::StopAll))
        );
    }
}
