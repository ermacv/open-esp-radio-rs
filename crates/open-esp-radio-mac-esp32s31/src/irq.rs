//! Allocation-free MAC ISR to Embassy-task event handoff.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_pac_esp32s31::MacInterruptRegisters;

pub const MAC_INT_TX_COMPLETE: u32 = 0x0000_0080;
pub const MAC_INT_COLLISION: u32 = 0x0000_0100;
pub const MAC_INT_WATCHDOG: u32 = 0x0000_0800;
pub const MAC_INT_RX_SUCCESS: u32 = 0x0000_4000;
pub const MAC_INT_TX_TIMEOUT: u32 = 0x0008_0000;

/// Finite MAC interrupt capability used by the hard ISR.
///
/// Production delegates both operations to generated PAC registers. Tests
/// can model the same ordering without receiving arbitrary MMIO identities.
pub trait MacInterrupt {
    fn snapshot(&mut self) -> (u32, u32);
    fn acknowledge(&mut self, events: u32);
}

impl MacInterrupt for MacInterruptRegisters {
    fn snapshot(&mut self) -> (u32, u32) {
        self.mac_interrupt_snapshot()
    }

    fn acknowledge(&mut self, events: u32) {
        self.acknowledge_mac_interrupts(events);
    }
}

pub const EVENT_TX_COMPLETE: u32 = 0x01;
pub const EVENT_TX_TIMEOUT: u32 = 0x02;
pub const EVENT_COLLISION: u32 = 0x04;
pub const EVENT_RX_SUCCESS: u32 = 0x08;
pub const EVENT_KNOWN_MASK: u32 = 0x0f;

pub const HANDLED_MAC_MASK: u32 =
    MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_RX_SUCCESS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqEvent {
    pub mac_pending: u32,
    pub event_mask: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqSnapshot {
    pub enabled: u32,
    pub status: u32,
    pub pending: u32,
    pub handled: u32,
    pub unhandled: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqDisposition {
    Posted,
    Spurious,
    Unhandled,
}

/// Duplicate interrupts coalesce by raw MAC bit, matching the C worker latch.
pub trait IrqSink {
    fn post(&self, mac_pending: u32);
    fn record_unhandled(&self, bits: u32);
}

/// Executor-neutral coalescing state shared by the ISR and its task.
///
/// The executor integration decides how a posted edge wakes the task. This
/// keeps the MAC crate independent of Embassy while retaining the exact
/// interrupt/event mapping.
pub struct IrqState {
    pending_mac: AtomicU32,
    observed_unhandled: AtomicU32,
}

impl IrqState {
    pub const fn new() -> Self {
        Self {
            pending_mac: AtomicU32::new(0),
            observed_unhandled: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn post(&self, mac_pending: u32) {
        self.pending_mac.fetch_or(mac_pending, Ordering::Release);
    }

    #[inline]
    pub fn record_unhandled(&self, bits: u32) {
        self.observed_unhandled.fetch_or(bits, Ordering::Relaxed);
    }

    pub fn observed_unhandled(&self) -> u32 {
        self.observed_unhandled.load(Ordering::Relaxed)
    }

    pub fn try_take(&self) -> Option<IrqEvent> {
        let mac_pending = self.pending_mac.swap(0, Ordering::AcqRel);
        (mac_pending != 0).then(|| IrqEvent {
            mac_pending,
            event_mask: event_mask(mac_pending),
        })
    }
}

impl IrqSink for IrqState {
    fn post(&self, mac_pending: u32) {
        self.post(mac_pending);
    }

    fn record_unhandled(&self, bits: u32) {
        self.record_unhandled(bits);
    }
}

impl Default for IrqState {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn event_mask(mac_pending: u32) -> u32 {
    let mut events = 0;
    if mac_pending & MAC_INT_TX_COMPLETE != 0 {
        events |= EVENT_TX_COMPLETE;
    }
    if mac_pending & MAC_INT_TX_TIMEOUT != 0 {
        events |= EVENT_TX_TIMEOUT;
    }
    if mac_pending & MAC_INT_COLLISION != 0 {
        events |= EVENT_COLLISION;
    }
    if mac_pending & MAC_INT_RX_SUCCESS != 0 {
        events |= EVENT_RX_SUCCESS;
    }
    events
}

/// Handles exactly one MAC status snapshot and posts only the four recovered
/// task-side events. The complete status snapshot is acknowledged before the
/// task can run, including unsupported bits, matching the pinned common ISR.
pub fn handle_mac_irq<M: MacInterrupt, S: IrqSink>(
    interrupt: &mut M,
    sink: &S,
) -> (IrqDisposition, IrqSnapshot) {
    let (status, enabled) = interrupt.snapshot();
    let handled = status & HANDLED_MAC_MASK;
    let unhandled = status & !HANDLED_MAC_MASK;
    let snapshot = IrqSnapshot {
        enabled,
        status,
        pending: status,
        handled,
        unhandled,
    };

    if status == 0 {
        return (IrqDisposition::Spurious, snapshot);
    }

    interrupt.acknowledge(status);
    sink.record_unhandled(unhandled);
    if handled == 0 {
        (IrqDisposition::Unhandled, snapshot)
    } else {
        sink.post(handled);
        (IrqDisposition::Posted, snapshot)
    }
}
