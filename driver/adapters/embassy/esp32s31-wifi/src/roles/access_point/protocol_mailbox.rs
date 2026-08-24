//! Typed protocol-to-radio actions for the AP RX ownership split.

/// Hardware actions inferred by the protocol consumer but executed only by
/// the radio/PAC owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointHardwareAction {
    ResetRxBlockAckWindow {
        hardware_index: u8,
        tid: u8,
        starting_sequence: u16,
        window: u16,
    },
}

/// Control actions inferred by RX protocol processing. These contain values,
/// never PAC references, descriptor owners, or borrowed frame bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointControlAction {
    ObservePeerActivity {
        peer: [u8; 6],
        at_micros: u64,
        power_state: Option<open_esp_radio_esp32s31_wifi_ap::protocol::ApPeerPowerState>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointProtocolAction {
    Hardware(Esp32s31AccessPointHardwareAction),
    Control(Esp32s31AccessPointControlAction),
}

/// Bounded value-only handoff from the protocol consumer to the radio owner.
///
/// The current AP executor owns both endpoints. The protocol half publishes
/// while it has no PAC or AP-engine capability; after that borrow ends, the
/// radio owner drains and executes the actions. Keeping the handoff explicit
/// permits moving the consumer to an independent task without changing the
/// action vocabulary.
pub struct Esp32s31AccessPointProtocolMailbox<const CAPACITY: usize> {
    actions: [Option<Esp32s31AccessPointProtocolAction>; CAPACITY],
    head: usize,
    len: usize,
}

impl<const CAPACITY: usize> Esp32s31AccessPointProtocolMailbox<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            actions: [None; CAPACITY],
            head: 0,
            len: 0,
        }
    }

    pub fn publisher(&mut self) -> Esp32s31AccessPointProtocolPublisher<'_, CAPACITY> {
        Esp32s31AccessPointProtocolPublisher { mailbox: self }
    }

    pub fn receiver(&mut self) -> Esp32s31AccessPointProtocolReceiver<'_, CAPACITY> {
        Esp32s31AccessPointProtocolReceiver { mailbox: self }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn remaining_capacity(&self) -> usize {
        CAPACITY.saturating_sub(self.len)
    }

    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }

    fn push(
        &mut self,
        action: Esp32s31AccessPointProtocolAction,
    ) -> Result<(), Esp32s31AccessPointProtocolAction> {
        if self.len == CAPACITY || CAPACITY == 0 {
            return Err(action);
        }
        let tail = (self.head + self.len) % CAPACITY;
        self.actions[tail] = Some(action);
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<Esp32s31AccessPointProtocolAction> {
        if self.len == 0 {
            return None;
        }
        let action = self.actions[self.head].take();
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        action
    }
}

impl<const CAPACITY: usize> Default for Esp32s31AccessPointProtocolMailbox<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side capability: it may publish values, but cannot execute them.
pub struct Esp32s31AccessPointProtocolPublisher<'mailbox, const CAPACITY: usize> {
    mailbox: &'mailbox mut Esp32s31AccessPointProtocolMailbox<CAPACITY>,
}

impl<const CAPACITY: usize> Esp32s31AccessPointProtocolPublisher<'_, CAPACITY> {
    pub fn try_publish(
        &mut self,
        action: Esp32s31AccessPointProtocolAction,
    ) -> Result<(), Esp32s31AccessPointProtocolAction> {
        self.mailbox.push(action)
    }
}

/// Radio-side capability: it may receive values, but cannot create protocol
/// conclusions or borrow their frame storage.
pub struct Esp32s31AccessPointProtocolReceiver<'mailbox, const CAPACITY: usize> {
    mailbox: &'mailbox mut Esp32s31AccessPointProtocolMailbox<CAPACITY>,
}

impl<const CAPACITY: usize> Esp32s31AccessPointProtocolReceiver<'_, CAPACITY> {
    pub fn try_receive(&mut self) -> Option<Esp32s31AccessPointProtocolAction> {
        self.mailbox.pop()
    }

    pub const fn len(&self) -> usize {
        self.mailbox.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.mailbox.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_preserves_typed_hardware_and_control_order() {
        let mut mailbox = Esp32s31AccessPointProtocolMailbox::<2>::new();
        let reset = Esp32s31AccessPointProtocolAction::Hardware(
            Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
                hardware_index: 2,
                tid: 6,
                starting_sequence: 0x345,
                window: 64,
            },
        );
        let activity = Esp32s31AccessPointProtocolAction::Control(
            Esp32s31AccessPointControlAction::ObservePeerActivity {
                peer: [1, 2, 3, 4, 5, 6],
                at_micros: 77,
                power_state: None,
            },
        );

        {
            let mut publisher = mailbox.publisher();
            publisher.try_publish(reset).unwrap();
            publisher.try_publish(activity).unwrap();
            assert_eq!(publisher.try_publish(reset), Err(reset));
        }
        let mut receiver = mailbox.receiver();
        assert_eq!(receiver.try_receive(), Some(reset));
        assert_eq!(receiver.try_receive(), Some(activity));
        assert_eq!(receiver.try_receive(), None);
    }
}
