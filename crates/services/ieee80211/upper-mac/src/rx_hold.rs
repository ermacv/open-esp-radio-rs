//! A service's received MSDUs on their way into the network.
//!
//! A service hands every MSDU on at once and goes on taking the port's
//! input, so beacons, EAPOL and its timers never wait for the network.
//! [`PortRxHold`] stands between it and a network whose receive queue may be
//! full: while the network takes no more, it keeps the MSDUs in order and
//! sends them as room appears. An MSDU in the port's buffer waits in that
//! buffer, up to `HELD` of them; the port's receive memory then runs short,
//! so a network that stays full makes the port's producer discard bulk data
//! before the frames that keep the link. An MSDU in parts (released late by
//! a reorder window, or one of an A-MSDU) borrows memory the hold cannot
//! keep, so it is copied into one of `COPIES` slots of
//! [`PORT_FRAME_CAPACITY`] octets. Beyond them, an MSDU is dropped and
//! counted.

use core::ops::Range;

use oer_ieee80211_lower_mac::{MacAddress, RxBuffer};
use oer_ieee80211_mac::data::EthernetFrameParts;

use crate::{client::PortMsdu, frame::PORT_FRAME_CAPACITY};

/// Why the network did not take a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkRefusal {
    /// It has no room now; the frame may go later.
    Full,
    /// It will never take this frame.
    Rejected,
}

/// What became of the MSDUs a [`PortRxHold`] was given.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortRxHoldCounters {
    /// Taken by the network.
    pub delivered: u32,
    /// Kept in the port's buffer while the network was full.
    pub held: u32,
    /// Copied while the network was full.
    pub copied: u32,
    /// Dropped with every buffer or every copy of its kind taken.
    pub dropped_full: u32,
    /// Refused by the network for good, or longer than a copy holds.
    pub rejected: u32,
}

/// An MSDU's Ethernet header fields.
#[derive(Clone, Copy)]
struct Header {
    destination: MacAddress,
    source: MacAddress,
    ether_type: u16,
}

impl Header {
    fn parts(self, payload: &[u8]) -> EthernetFrameParts<'_> {
        EthernetFrameParts {
            destination: self.destination,
            source: self.source,
            ether_type: self.ether_type,
            payload,
        }
    }
}

/// An MSDU waiting in the port's buffer.
struct Held<B> {
    order: u32,
    buffer: B,
    header: Header,
    payload: Range<usize>,
}

/// An MSDU waiting as a copy.
#[derive(Clone, Copy)]
struct Copied {
    order: u32,
    header: Header,
    len: usize,
}

/// Received MSDUs waiting for the network, in order: up to `HELD` in the
/// port's buffers and `COPIES` copied.
pub struct PortRxHold<B, const HELD: usize, const COPIES: usize> {
    /// The buffers, oldest at `head`.
    held: [Option<Held<B>>; HELD],
    head: usize,
    held_len: usize,
    copied: [Option<Copied>; COPIES],
    copies: [[u8; PORT_FRAME_CAPACITY]; COPIES],
    /// The order the next waiting MSDU takes.
    next_order: u32,
    counters: PortRxHoldCounters,
}

impl<B: RxBuffer, const HELD: usize, const COPIES: usize> Default for PortRxHold<B, HELD, COPIES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: RxBuffer, const HELD: usize, const COPIES: usize> PortRxHold<B, HELD, COPIES> {
    pub const fn new() -> Self {
        Self {
            held: [const { None }; HELD],
            head: 0,
            held_len: 0,
            copied: [None; COPIES],
            copies: [[0; PORT_FRAME_CAPACITY]; COPIES],
            next_order: 0,
            counters: PortRxHoldCounters {
                delivered: 0,
                held: 0,
                copied: 0,
                dropped_full: 0,
                rejected: 0,
            },
        }
    }

    /// MSDUs waiting for the network.
    pub fn len(&self) -> usize {
        self.held_len + self.copied.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn counters(&self) -> PortRxHoldCounters {
        self.counters
    }

    /// Hand `msdu` to the network through `send`, after the MSDUs waiting
    /// before it; keep it in order while the network is full.
    pub fn deliver(
        &mut self,
        msdu: PortMsdu<'_, B>,
        send: &mut impl FnMut(EthernetFrameParts<'_>) -> Result<(), NetworkRefusal>,
    ) {
        self.flush(send);
        let waiting = !self.is_empty();
        match msdu {
            PortMsdu::Buffer {
                buffer,
                destination,
                source,
                ether_type,
                payload,
            } => {
                let header = Header {
                    destination,
                    source,
                    ether_type,
                };
                if !waiting {
                    let bytes = buffer.bytes().get(payload.clone()).unwrap_or_default();
                    match send(header.parts(bytes)) {
                        Ok(()) => {
                            self.counters.delivered += 1;
                            return;
                        }
                        Err(NetworkRefusal::Rejected) => {
                            self.counters.rejected += 1;
                            return;
                        }
                        Err(NetworkRefusal::Full) => {}
                    }
                }
                if self.held_len == HELD {
                    self.counters.dropped_full += 1;
                    return;
                }
                let order = self.take_order();
                let slot = (self.head + self.held_len) % HELD;
                self.held[slot] = Some(Held {
                    order,
                    buffer,
                    header,
                    payload,
                });
                self.held_len += 1;
                self.counters.held += 1;
            }
            PortMsdu::Parts(parts) => {
                if !waiting {
                    match send(parts) {
                        Ok(()) => {
                            self.counters.delivered += 1;
                            return;
                        }
                        Err(NetworkRefusal::Rejected) => {
                            self.counters.rejected += 1;
                            return;
                        }
                        Err(NetworkRefusal::Full) => {}
                    }
                }
                let Some(slot) = self.copied.iter().position(Option::is_none) else {
                    self.counters.dropped_full += 1;
                    return;
                };
                let Some(copy) = self.copies[slot].get_mut(..parts.payload.len()) else {
                    self.counters.rejected += 1;
                    return;
                };
                copy.copy_from_slice(parts.payload);
                let order = self.take_order();
                self.copied[slot] = Some(Copied {
                    order,
                    header: Header {
                        destination: parts.destination,
                        source: parts.source,
                        ether_type: parts.ether_type,
                    },
                    len: parts.payload.len(),
                });
                self.counters.copied += 1;
            }
        }
    }

    /// Send the waiting MSDUs, oldest first, while the network takes them.
    pub fn flush(
        &mut self,
        send: &mut impl FnMut(EthernetFrameParts<'_>) -> Result<(), NetworkRefusal>,
    ) {
        loop {
            let held_order = (self.held_len > 0)
                .then(|| self.held[self.head].as_ref().map(|held| held.order))
                .flatten();
            let copy = self
                .copied
                .iter()
                .enumerate()
                .filter_map(|(slot, copied)| copied.map(|copied| (slot, copied)))
                .min_by_key(|(_, copied)| copied.order.wrapping_sub(self.next_order));
            // The older of the two goes first; orders count on from the
            // oldest waiting, so the older is the one further behind next.
            let take_copy = match (held_order, copy) {
                (None, None) => return,
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some(held), Some((_, copied))) => {
                    copied.order.wrapping_sub(self.next_order) < held.wrapping_sub(self.next_order)
                }
            };
            let sent = match (take_copy, copy) {
                (true, Some((slot, copied))) => {
                    send(copied.header.parts(&self.copies[slot][..copied.len]))
                }
                _ => {
                    let Some(held) = self.held[self.head].as_ref() else {
                        return;
                    };
                    send(
                        held.header.parts(
                            held.buffer
                                .bytes()
                                .get(held.payload.clone())
                                .unwrap_or_default(),
                        ),
                    )
                }
            };
            match sent {
                Ok(()) => self.counters.delivered += 1,
                Err(NetworkRefusal::Full) => return,
                Err(NetworkRefusal::Rejected) => self.counters.rejected += 1,
            }
            match (take_copy, copy) {
                (true, Some((slot, _))) => self.copied[slot] = None,
                _ => {
                    // The buffer goes back to the port.
                    self.held[self.head] = None;
                    self.head = (self.head + 1) % HELD;
                    self.held_len -= 1;
                }
            }
        }
    }

    fn take_order(&mut self) -> u32 {
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        order
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use std::{rc::Rc, vec, vec::Vec};

    use super::*;

    /// A port buffer that counts its return.
    struct Buffer(Vec<u8>, Rc<Cell<usize>>);

    impl RxBuffer for Buffer {
        fn bytes(&self) -> &[u8] {
            &self.0
        }
    }

    impl Drop for Buffer {
        fn drop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
    }

    fn msdu(payload: u8, returned: &Rc<Cell<usize>>) -> PortMsdu<'static, Buffer> {
        PortMsdu::Buffer {
            buffer: Buffer(vec![0xff, payload], returned.clone()),
            destination: [2; 6],
            source: [4; 6],
            ether_type: 0x0800,
            payload: 1..2,
        }
    }

    fn parts(payload: &[u8]) -> PortMsdu<'_, Buffer> {
        PortMsdu::Parts(EthernetFrameParts {
            destination: [2; 6],
            source: [4; 6],
            ether_type: 0x0800,
            payload,
        })
    }

    /// A network with `room` places that records what it takes.
    struct Network {
        room: usize,
        taken: Vec<u8>,
    }

    impl Network {
        fn send(&mut self, parts: EthernetFrameParts<'_>) -> Result<(), NetworkRefusal> {
            if parts.payload == [0xee] {
                return Err(NetworkRefusal::Rejected);
            }
            if self.room == 0 {
                return Err(NetworkRefusal::Full);
            }
            self.room -= 1;
            self.taken.push(parts.payload[0]);
            Ok(())
        }
    }

    #[test]
    fn a_full_network_holds_the_port_s_buffers_and_takes_them_in_order() {
        let returned = Rc::new(Cell::new(0));
        let mut hold = PortRxHold::<Buffer, 2, 0>::new();
        let mut network = Network {
            room: 1,
            taken: Vec::new(),
        };
        for payload in 1..=4 {
            hold.deliver(msdu(payload, &returned), &mut |parts| network.send(parts));
        }
        // One went at once, two wait, the fourth found every buffer taken.
        assert_eq!(network.taken, [1]);
        assert_eq!(hold.len(), 2);
        assert_eq!(returned.get(), 2);
        // Room: the held ones go out, oldest first, and their buffers back.
        network.room = 8;
        hold.flush(&mut |parts| network.send(parts));
        assert_eq!(network.taken, [1, 2, 3]);
        assert!(hold.is_empty());
        assert_eq!(returned.get(), 4);
        assert_eq!(
            hold.counters(),
            PortRxHoldCounters {
                delivered: 3,
                held: 2,
                copied: 0,
                dropped_full: 1,
                rejected: 0,
            }
        );
    }

    #[test]
    fn parts_a_reorder_window_releases_are_copied_and_keep_their_order() {
        let returned = Rc::new(Cell::new(0));
        let mut hold = PortRxHold::<Buffer, 2, 2>::new();
        let mut network = Network {
            room: 0,
            taken: Vec::new(),
        };
        // A buffer waits, then a run of three parts a window released: two
        // fit the copies, the third finds every copy taken.
        hold.deliver(msdu(1, &returned), &mut |parts| network.send(parts));
        for payload in [[2], [3], [4]] {
            hold.deliver(parts(&payload), &mut |parts| network.send(parts));
        }
        assert_eq!(hold.len(), 3);
        // A buffer after them keeps its place behind them.
        hold.deliver(msdu(5, &returned), &mut |parts| network.send(parts));
        network.room = 8;
        hold.flush(&mut |parts| network.send(parts));
        assert_eq!(network.taken, [1, 2, 3, 5]);
        assert_eq!(
            hold.counters(),
            PortRxHoldCounters {
                delivered: 4,
                held: 2,
                copied: 2,
                dropped_full: 1,
                rejected: 0,
            }
        );
        // The copies are free again, and the ring wraps.
        for payload in [[6], [7]] {
            network.room = 0;
            hold.deliver(parts(&payload), &mut |parts| network.send(parts));
        }
        network.room = 8;
        hold.flush(&mut |parts| network.send(parts));
        assert_eq!(network.taken, [1, 2, 3, 5, 6, 7]);
        assert_eq!(returned.get(), 2);
    }

    #[test]
    fn a_frame_the_network_refuses_for_good_is_counted_and_released() {
        let returned = Rc::new(Cell::new(0));
        let mut hold = PortRxHold::<Buffer, 2, 1>::new();
        let mut network = Network {
            room: 4,
            taken: Vec::new(),
        };
        hold.deliver(msdu(0xee, &returned), &mut |parts| network.send(parts));
        hold.deliver(parts(&[0xee]), &mut |parts| network.send(parts));
        // A copy too long for its slot is refused too.
        let long = vec![0; PORT_FRAME_CAPACITY + 1];
        network.room = 0;
        hold.deliver(parts(&long), &mut |parts| network.send(parts));
        assert_eq!(hold.counters().rejected, 3);
        assert!(hold.is_empty());
        assert_eq!(returned.get(), 1);
    }
}
