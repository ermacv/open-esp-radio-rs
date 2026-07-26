use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    adapter::{give_internal_semaphore, try_lock_internal_mutex, unlock_internal_mutex},
    context::TaskContextGuard,
};

const EAP_COUNTERS_OFFSET: usize = 0x107;
const EAP_STATE_OFFSET: usize = 0x10a;
const EAP_PMK_OFFSET: usize = 0x118;
const EAPOL_STARTED_OFFSET: usize = 0x128;
const ETH_P_EAPOL: u16 = 0x888e;

pub(crate) const EAP_START_EVENT: u32 = u32::MAX - 16;
pub(crate) const EAP_RX_EVENT: u32 = u32::MAX - 15;
pub(crate) const EAP_STOP_EVENT: u32 = u32::MAX - 14;
pub(crate) const EAP_RX_CONTINUATION: u32 = u32::MAX - 13;

static QUEUE_ACTIVE: AtomicBool = AtomicBool::new(false);
static TASK_ACTIVE: AtomicBool = AtomicBool::new(false);
static EMPTY_EAPOL_PAYLOAD: [u8; 4] = [0; 4];
static EAP_SUCCESS: &[u8] = b"EAP Success\0";

#[repr(C)]
struct RxQueue {
    head: *mut RxNode,
    tail_link: *mut *mut RxNode,
}

#[repr(C)]
struct RxNode {
    private: [u8; 12],
    data: *mut u8,
    length: usize,
    next: *mut RxNode,
}

type EloopCallback = unsafe extern "C" fn(*mut c_void, *mut c_void);

unsafe extern "C" {
    static mut __esp_s_wpa2_rxq: RxQueue;
    static __esp_s_wpa2_rxq_end: u8;
    static mut __esp_s_wifi_wpa2_sync_sem: *mut c_void;
    static __esp_s_wifi_wpa2_sync_sem_end: u8;
    static mut __esp_s_wpa2_queue: *mut c_void;
    static __esp_s_wpa2_queue_end: u8;
    static mut __esp_g_eap_sm: *mut c_void;
    static __esp_g_eap_sm_end: u8;
    static mut __esp_s_wpa2_data_lock: *mut c_void;
    static __esp_s_wpa2_data_lock_end: u8;

    fn wpa2_task(parameter: *mut c_void);
    fn __esp_eap_start_eapol(context: *mut c_void, data: *mut c_void);
    fn __esp_eap_start_eapol_end();
    fn __esp_wpa2_set_eap_state(state: i32);
    fn __esp_wpa2_set_eap_state_end();

    fn wpa_sta_cur_pmksa_matches_akm() -> bool;
    fn wpa_sta_is_cur_pmksa_set() -> bool;
    fn esp_wifi_get_assoc_bssid_internal(bssid: *mut u8) -> i32;
    fn wpa_alloc_eapol(
        sm: *mut c_void,
        packet_type: u8,
        data: *const c_void,
        data_len: u16,
        message_len: *mut usize,
        data_pos: *mut *mut c_void,
    ) -> *mut u8;
    fn wpa_ether_send(
        sm: *mut c_void,
        destination: *const u8,
        protocol: u16,
        data: *const u8,
        data_len: usize,
    ) -> i32;
    fn wpa_free_eapol(buffer: *mut u8);
    fn eloop_cancel_timeout(
        callback: EloopCallback,
        eloop_data: *mut c_void,
        user_data: *mut c_void,
    ) -> i32;
    fn wpabuf_alloc_copy(data: *const c_void, length: usize) -> *mut c_void;
    fn wpabuf_free(buffer: *mut c_void);
    fn eap_sm_process_request(sm: *mut c_void, request: *mut c_void) -> i32;
    fn wpa_set_pmk(pmk: *mut u8, length: usize, pmkid: *const u8, external: bool);
    fn eap_deinit_prev_method(sm: *mut c_void, reason: *const u8);
    fn free(allocation: *mut c_void);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DispatchResult {
    Complete,
    Deferred,
    ContinueRx,
}

pub(crate) fn try_create_queue(length: u32, item_size: u32) -> Option<*mut c_void> {
    if length != 3 || item_size as usize != core::mem::size_of::<crate::event::PpEvent>() {
        return None;
    }
    QUEUE_ACTIVE.store(true, Ordering::Release);
    Some(queue_handle())
}

pub(crate) fn queue_handle() -> *mut c_void {
    ptr::addr_of!(QUEUE_ACTIVE).cast_mut().cast()
}

pub(crate) fn task_handle() -> *mut c_void {
    ptr::addr_of!(TASK_ACTIVE).cast_mut().cast()
}

pub(crate) fn is_queue(handle: *mut c_void) -> bool {
    handle == queue_handle() && QUEUE_ACTIVE.load(Ordering::Acquire)
}

pub(crate) fn delete_queue(handle: *mut c_void) -> bool {
    if handle != queue_handle() {
        return false;
    }
    QUEUE_ACTIVE.store(false, Ordering::Release);
    true
}

pub(crate) unsafe fn try_start_task(entry: *mut c_void, out_handle: *mut *mut c_void) -> bool {
    if entry != wpa2_task as *const () as *mut c_void || !local_layout_matches() {
        return false;
    }
    if !out_handle.is_null() {
        out_handle.write(task_handle());
    }
    TASK_ACTIVE.store(true, Ordering::Release);
    true
}

pub(crate) fn is_task_handle(handle: *mut c_void) -> bool {
    handle == task_handle()
}

pub(crate) unsafe fn is_sync_semaphore(handle: *mut c_void) -> bool {
    !handle.is_null() && ptr::addr_of!(__esp_s_wifi_wpa2_sync_sem).read_volatile() == handle
}

pub(crate) fn stop_task() {
    TASK_ACTIVE.store(false, Ordering::Release);
}

pub(crate) fn encode_vendor_signal(signal: u32) -> Option<u32> {
    match signal {
        0 => Some(EAP_START_EVENT),
        1 => Some(EAP_RX_EVENT),
        2 => Some(EAP_STOP_EVENT),
        _ => None,
    }
}

pub(crate) fn is_async_work(kind: u32) -> bool {
    matches!(kind, EAP_START_EVENT | EAP_RX_EVENT | EAP_RX_CONTINUATION)
}

pub(crate) unsafe fn dispatch(kind: u32) -> DispatchResult {
    let _context = TaskContextGuard::enter(task_handle(), kind);
    let (signal, account) = match kind {
        EAP_START_EVENT => (0, true),
        EAP_RX_EVENT => (1, true),
        EAP_STOP_EVENT => (2, true),
        EAP_RX_CONTINUATION => (1, false),
        _ => return DispatchResult::Complete,
    };

    let sm = ptr::addr_of!(__esp_g_eap_sm).read_volatile();
    if sm.is_null() {
        if signal == 2 {
            finish_stop();
        }
        return DispatchResult::Complete;
    }

    if signal != 1 && account && !account_signal(sm.cast(), signal) {
        return DispatchResult::Deferred;
    }

    match signal {
        0 => {
            process_start(sm);
            DispatchResult::Complete
        }
        1 => process_one_rx(sm, account),
        2 => {
            finish_stop();
            DispatchResult::Complete
        }
        _ => DispatchResult::Complete,
    }
}

unsafe fn local_layout_matches() -> bool {
    section_size(
        __esp_eap_start_eapol as *const (),
        __esp_eap_start_eapol_end as *const (),
    ) == 0x0c
        && section_size(
            __esp_wpa2_set_eap_state as *const (),
            __esp_wpa2_set_eap_state_end as *const (),
        ) == 0x18
        && object_size(
            ptr::addr_of!(__esp_s_wpa2_rxq),
            ptr::addr_of!(__esp_s_wpa2_rxq_end),
        ) == 8
        && object_size(
            ptr::addr_of!(__esp_s_wifi_wpa2_sync_sem),
            ptr::addr_of!(__esp_s_wifi_wpa2_sync_sem_end),
        ) == 4
        && object_size(
            ptr::addr_of!(__esp_s_wpa2_queue),
            ptr::addr_of!(__esp_s_wpa2_queue_end),
        ) == 4
        && object_size(
            ptr::addr_of!(__esp_g_eap_sm),
            ptr::addr_of!(__esp_g_eap_sm_end),
        ) == 4
        && object_size(
            ptr::addr_of!(__esp_s_wpa2_data_lock),
            ptr::addr_of!(__esp_s_wpa2_data_lock_end),
        ) == 4
}

fn section_size(start: *const (), end: *const ()) -> usize {
    (end as usize).wrapping_sub(start as usize)
}

fn object_size<T, U>(start: *const T, end: *const U) -> usize {
    (end as usize).wrapping_sub(start as usize)
}

unsafe fn account_signal(sm: *mut u8, signal: usize) -> bool {
    let lock = ptr::addr_of!(__esp_s_wpa2_data_lock).read_volatile();
    if lock.is_null() || !try_lock_internal_mutex(lock) {
        return false;
    }
    let counter = sm.add(EAP_COUNTERS_OFFSET + signal);
    let value = counter.read_volatile();
    if value != 0 {
        counter.write_volatile(value - 1);
    }
    let _ = unlock_internal_mutex(lock);
    true
}

unsafe fn process_start(sm: *mut c_void) {
    if wpa_sta_cur_pmksa_matches_akm() && wpa_sta_is_cur_pmksa_set() {
        return;
    }

    let mut bssid = [0u8; 6];
    if esp_wifi_get_assoc_bssid_internal(bssid.as_mut_ptr()) != 0 {
        return;
    }

    let mut message_len = 0usize;
    let frame = wpa_alloc_eapol(
        sm,
        1,
        EMPTY_EAPOL_PAYLOAD.as_ptr().cast(),
        0,
        ptr::from_mut(&mut message_len),
        ptr::null_mut(),
    );
    if frame.is_null() {
        return;
    }
    __esp_wpa2_set_eap_state(1);
    wpa_ether_send(sm, bssid.as_ptr(), ETH_P_EAPOL, frame, message_len);
    wpa_free_eapol(frame);
}

unsafe fn process_one_rx(sm: *mut c_void, account: bool) -> DispatchResult {
    let lock = ptr::addr_of!(__esp_s_wpa2_data_lock).read_volatile();
    if lock.is_null() || !try_lock_internal_mutex(lock) {
        return DispatchResult::Deferred;
    }

    if account {
        let counter = sm.cast::<u8>().add(EAP_COUNTERS_OFFSET + 1);
        let value = counter.read_volatile();
        if value != 0 {
            counter.write_volatile(value - 1);
        }
    }
    let (node, more) = pop_rx_node_locked();
    let _ = unlock_internal_mutex(lock);
    if node.is_null() {
        return DispatchResult::Complete;
    }

    let state = sm.cast::<u8>();
    if state.add(EAPOL_STARTED_OFFSET).read_volatile() == 0 {
        state.add(EAPOL_STARTED_OFFSET).write_volatile(1);
        eloop_cancel_timeout(
            __esp_eap_start_eapol as EloopCallback,
            ptr::null_mut(),
            ptr::null_mut(),
        );
    }

    let data = (*node).data;
    let length = (*node).length;
    if !data.is_null() && valid_eapol(data, length) {
        match data.add(4).read() {
            1 => process_eap_request(state, data.add(4), length - 4),
            3 => process_eap_success(state),
            4 => __esp_wpa2_set_eap_state(3),
            _ => {}
        }
    }

    free(data.cast());
    free(node.cast());
    if more {
        DispatchResult::ContinueRx
    } else {
        DispatchResult::Complete
    }
}

unsafe fn pop_rx_node_locked() -> (*mut RxNode, bool) {
    let queue = ptr::addr_of_mut!(__esp_s_wpa2_rxq);
    let node = ptr::addr_of!((*queue).head).read_volatile();
    let more = if node.is_null() {
        false
    } else {
        let next = ptr::addr_of!((*node).next).read_volatile();
        ptr::addr_of_mut!((*queue).head).write_volatile(next);
        if next.is_null() {
            ptr::addr_of_mut!((*queue).tail_link).write_volatile(ptr::addr_of_mut!((*queue).head));
        }
        ptr::addr_of_mut!((*node).next).write_volatile(ptr::null_mut());
        !next.is_null()
    };
    (node, more)
}

unsafe fn valid_eapol(data: *const u8, length: usize) -> bool {
    if length < 4 || data.add(1).read() != 0 {
        return false;
    }
    let declared = u16::from_be_bytes([data.add(2).read(), data.add(3).read()]) as usize;
    declared >= 4 && declared <= length - 4
}

unsafe fn process_eap_request(sm: *mut u8, request: *const u8, length: usize) {
    if sm.add(EAP_STATE_OFFSET).read_volatile() == 2 {
        __esp_wpa2_set_eap_state(1);
    }
    let request = wpabuf_alloc_copy(request.cast(), length);
    eap_sm_process_request(sm.cast(), request);
    wpabuf_free(request);
}

unsafe fn process_eap_success(sm: *mut u8) {
    let pmk_slot = sm.add(EAP_PMK_OFFSET).cast::<*mut u8>();
    let pmk = pmk_slot.read_volatile();
    if pmk.is_null() {
        __esp_wpa2_set_eap_state(3);
        return;
    }

    wpa_set_pmk(pmk, 0, ptr::null(), false);
    free(pmk.cast());
    pmk_slot.write_volatile(ptr::null_mut());
    __esp_wpa2_set_eap_state(2);
    eap_deinit_prev_method(sm.cast(), EAP_SUCCESS.as_ptr());
}

unsafe fn finish_stop() {
    let queue = ptr::addr_of!(__esp_s_wpa2_queue).read_volatile();
    if !queue.is_null() {
        delete_queue(queue);
    }
    ptr::addr_of_mut!(__esp_s_wpa2_queue).write_volatile(ptr::null_mut());
    stop_task();

    let semaphore = ptr::addr_of!(__esp_s_wifi_wpa2_sync_sem).read_volatile();
    if !semaphore.is_null() {
        let _ = give_internal_semaphore(semaphore);
    }
}
