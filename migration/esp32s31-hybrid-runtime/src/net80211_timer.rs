use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::diagnostics::BlockingCall;
use crate::event::PpEvent;

pub(crate) const NET80211_TIMER_EVENT: u32 = 0xffff_ff70;
const TIMER_SLOT_CAPACITY: usize = 16;
const TIMER_SLOT_MASK: usize = (1 << TIMER_SLOT_CAPACITY) - 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct TimerEnvelope {
    id: u8,
    padding: [u8; 3],
    argument: *mut c_void,
}

#[repr(C)]
struct TimerSlot {
    envelope: UnsafeCell<TimerEnvelope>,
}

unsafe impl Sync for TimerSlot {}

impl TimerSlot {
    const fn new() -> Self {
        Self {
            envelope: UnsafeCell::new(TimerEnvelope {
                id: 0,
                padding: [0; 3],
                argument: ptr::null_mut(),
            }),
        }
    }
}

static TIMER_SLOTS: [TimerSlot; TIMER_SLOT_CAPACITY] =
    [const { TimerSlot::new() }; TIMER_SLOT_CAPACITY];
static CLAIMED_TIMER_SLOTS: AtomicUsize = AtomicUsize::new(0);
static REJECTED_TIMER_EVENTS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn ieee80211_timer_process(kind: u32, id: u32, argument: *mut c_void) -> i32;
    fn __real_ieee80211_timer_process(kind: u32, id: u32, argument: *mut c_void) -> i32;
    fn ieee80211_hostap_send_beacon_process();
    fn hostap_handle_timer_process(peer: *mut c_void);
    fn cnx_auth_timeout_process();
    fn cnx_assoc_timeout_process();
    fn cnx_connect_next_ap_timeout_process();
}

pub(crate) fn timer_process_link_wrapper_active() -> bool {
    core::ptr::eq(
        ieee80211_timer_process as *const (),
        __wrap_ieee80211_timer_process as *const (),
    )
}

const fn supported_strict_timer(id: u8) -> bool {
    matches!(id, 0 | 8 | 9 | 11 | 12 | 13 | 44)
}

fn claim_slot() -> Option<usize> {
    let claimed = CLAIMED_TIMER_SLOTS.load(Ordering::Acquire);
    let free = !claimed & TIMER_SLOT_MASK;
    if free == 0 {
        return None;
    }
    let index = free.trailing_zeros() as usize;
    let bit = 1_usize << index;
    CLAIMED_TIMER_SLOTS
        .compare_exchange(claimed, claimed | bit, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| index)
}

fn release_slot(index: usize) {
    CLAIMED_TIMER_SLOTS.fetch_and(!(1_usize << index), Ordering::AcqRel);
}

fn slot_index(argument: *mut c_void) -> Option<usize> {
    let base = ptr::addr_of!(TIMER_SLOTS) as usize;
    let address = argument as usize;
    let stride = mem::size_of::<TimerSlot>();
    let offset = address.checked_sub(base)?;
    if stride == 0 || offset % stride != 0 {
        return None;
    }
    let index = offset / stride;
    (index < TIMER_SLOT_CAPACITY).then_some(index)
}

fn enqueue_strict_timer(id: u8, argument: *mut c_void) -> bool {
    let Some(index) = claim_slot() else {
        return false;
    };
    let slot = &TIMER_SLOTS[index];
    unsafe {
        slot.envelope.get().write(TimerEnvelope {
            id,
            padding: [0; 3],
            argument,
        });
    }
    let queued = crate::adapter::enqueue_internal_event(PpEvent {
        kind: NET80211_TIMER_EVENT,
        argument: slot.envelope.get().cast(),
    });
    if !queued {
        release_slot(index);
    }
    queued
}

/// Publish the first AP beacon continuation after cold takeover.
///
/// The S31 AP start path creates both beacon buffers and registers its TX
/// completion callback, but does not arm the first OSI timer without its
/// original task lifecycle. One bounded internal event supplies that missing
/// edge. Every later beacon is rearmed by the beacon TX completion callback on
/// the same async timer pool.
pub fn request_initial_ap_beacon() -> bool {
    crate::critical::strict_wifi_hart_armed()
        && crate::critical::on_strict_wifi_hart()
        && enqueue_strict_timer(9, ptr::null_mut())
}

/// Final-link replacement for the heap-owning timer-event producer.
///
/// Before the strict proof this delegates to the original initialization
/// path. Afterwards it claims one fixed envelope and posts one executor event.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_timer_process(
    kind: u32,
    id: u32,
    argument: *mut c_void,
) -> i32 {
    if !crate::critical::strict_wifi_hart_armed() {
        return __real_ieee80211_timer_process(kind, id, argument);
    }
    if !crate::critical::on_strict_wifi_hart()
        || kind != 7
        || id > u32::from(u8::MAX)
        || !supported_strict_timer(id as u8)
    {
        REJECTED_TIMER_EVENTS.fetch_add(1, Ordering::Relaxed);
        crate::adapter::blocking_probe().record(
            BlockingCall::Net80211TimerRejected,
            kind,
            id as usize,
        );
        return -1;
    }
    if !enqueue_strict_timer(id as u8, argument) {
        REJECTED_TIMER_EVENTS.fetch_add(1, Ordering::Relaxed);
        crate::adapter::blocking_probe().record(
            BlockingCall::Net80211TimerRejected,
            kind,
            id as usize,
        );
        return -1;
    }
    0
}

pub(crate) unsafe fn dispatch(argument: *mut c_void) -> Result<(), Net80211TimerError> {
    let Some(index) = slot_index(argument) else {
        return Err(Net80211TimerError::InvalidSlot);
    };
    let bit = 1_usize << index;
    if CLAIMED_TIMER_SLOTS.load(Ordering::Acquire) & bit == 0 {
        return Err(Net80211TimerError::InvalidSlot);
    }
    let envelope = TIMER_SLOTS[index].envelope.get();
    let id = (*envelope).id;
    if !supported_strict_timer(id) {
        release_slot(index);
        return Err(Net80211TimerError::UnsupportedId(id));
    }

    let original_argument = (*envelope).argument;
    release_slot(index);
    match id {
        // `ieee80211_timer_connect` only returns success.
        0 => Ok(()),
        // One dwell may have been armed by the connect request immediately
        // before cold handoff. The channel module validates the pinned scan
        // callback and accepts this bridge exactly once.
        8 => crate::channel_switch::complete_legacy_scan_dwell(original_argument as usize)
            .map_err(Net80211TimerError::ChannelSwitch),
        // The AP beacon timer's producer carries no state in its argument.
        // Run the finite beacon preparation/transmit leaf directly on the
        // radio-owner stack, without recreating the heap/API-lock envelope in
        // `ieee80211_timer_process`.
        9 => {
            ieee80211_hostap_send_beacon_process();
            Ok(())
        }
        // Authentication and association retries are ordinary executor timer
        // continuations. Call their finite state-machine leaves directly,
        // bypassing the heap-owning timer envelope in the vendor producer.
        11 => {
            cnx_auth_timeout_process();
            Ok(())
        }
        // `ieee80211_register_hostap_timer` replaces table id 12 with the AP
        // peer lifecycle leaf. Preserve its peer argument in the fixed timer
        // envelope and run it on the sole radio-owner stack.
        12 => {
            hostap_handle_timer_process(original_argument);
            Ok(())
        }
        13 => {
            cnx_assoc_timeout_process();
            Ok(())
        }
        44 => {
            cnx_connect_next_ap_timeout_process();
            Ok(())
        }
        _ => Err(Net80211TimerError::UnsupportedId(id)),
    }
}

pub fn rejected_net80211_timer_events() -> usize {
    REJECTED_TIMER_EVENTS.load(Ordering::Acquire)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Net80211TimerError {
    InvalidSlot,
    UnsupportedId(u8),
    ChannelSwitch(crate::channel_switch::ChannelSwitchError),
}

const _: () = assert!(mem::size_of::<TimerEnvelope>() == 8);
