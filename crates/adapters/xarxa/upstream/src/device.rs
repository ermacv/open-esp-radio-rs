use crate::{Driver, Endpoint, PacketBuf, QueuedPacket, driver};
use core::task::Waker;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::TrySendError;

/// One upstream driver. Only this owner can publish TX or consume RX.
pub struct Device<'a, M: RawMutex, const RX: usize, const TX: usize> {
    endpoint: Endpoint<'a, M, RX, TX>,
    address: [u8; 6],
    admitted_epoch: Option<u32>,
    rx_remaining: usize,
}

impl<'a, M: RawMutex, const RX: usize, const TX: usize> Device<'a, M, RX, TX> {
    pub(super) fn new(endpoint: Endpoint<'a, M, RX, TX>, address: [u8; 6]) -> Self {
        Self {
            endpoint,
            address,
            admitted_epoch: None,
            rx_remaining: RX,
        }
    }
}

impl<M: RawMutex, const RX: usize, const TX: usize> Driver for Device<'_, M, RX, TX> {
    fn capabilities(&self) -> driver::Capabilities {
        // No hardware checksum offload is advertised.
        driver::Capabilities::default()
    }

    fn hardware_address(&self) -> driver::HardwareAddress {
        driver::HardwareAddress::Ethernet(self.address)
    }

    fn link_state(&mut self) -> driver::LinkState {
        if self.endpoint.resources.epoch() & 1 != 0 {
            driver::LinkState::Up
        } else {
            driver::LinkState::Down
        }
    }

    fn register_waker(&mut self, waker: &Waker) -> Result<(), driver::NotSupported> {
        self.endpoint.resources.network_waker.register(waker);
        Ok(())
    }

    fn receive(&mut self) -> Option<PacketBuf> {
        let resources = self.endpoint.resources;
        loop {
            // Upstream drains receive() until None. Bound one drain even if
            // the other core keeps refilling this queue, then schedule the
            // continuation so socket tasks can consume the received owners.
            if self.rx_remaining == 0 {
                self.rx_remaining = RX;
                if !resources.rx.is_empty() {
                    resources.network_waker.wake();
                }
                return None;
            }
            let Ok(queued) = resources.rx.try_receive() else {
                self.rx_remaining = RX;
                return None;
            };
            self.rx_remaining -= 1;
            let epoch = resources.epoch();
            if epoch & 1 != 0 && epoch == queued.epoch {
                return Some(queued.packet);
            }
        }
    }

    fn can_transmit(&mut self) -> bool {
        let epoch = self.endpoint.resources.epoch();
        let available = epoch & 1 != 0 && !self.endpoint.resources.tx.is_full();
        self.admitted_epoch = available.then_some(epoch);
        available
    }

    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        let resources = self.endpoint.resources;
        let epoch = self
            .admitted_epoch
            .take()
            .unwrap_or_else(|| resources.epoch());
        if epoch & 1 == 0 {
            return Err(packet);
        }
        // Honor a preceding successful can_transmit even across link teardown.
        // The old epoch is retained; this packet cannot enter a new association.
        if epoch != resources.epoch() {
            return Ok(());
        }
        match resources.tx.try_send(QueuedPacket { epoch, packet }) {
            Ok(()) => {
                resources.published.signal(());
                Ok(())
            }
            Err(TrySendError::Full(queued)) => Err(queued.packet),
        }
    }
}
