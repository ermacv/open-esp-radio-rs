use core::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    adapter::{cancel_internal_timer, schedule_internal_timer},
    timer::RawOsiTimer,
};

const WPA_SM_SIZE: usize = 0x488;
const COUNTERMEASURES_OFFSET: usize = 0x360;
const MIC_FAILURES_OFFSET: usize = 0x434;
const MIC_CALLBACK_OFFSET: usize = 0x44;
const MIC_SETTLE_DELAY_US: u32 = 10_000;
const COUNTERMEASURE_TIMEOUT_SECONDS: u32 = 60;

type EloopCallback = unsafe extern "C" fn(*mut c_void, *mut c_void);
type MichaelCallback = unsafe extern "C" fn(u16) -> i32;

unsafe extern "C" {
    static mut gWpaSm: [u8; WPA_SM_SIZE];
    static mut wpa_cb: *mut c_void;

    fn wpa_michael_mic_failure(isunicast: u16) -> i32;
    fn wpa_sm_set_state(state: i32);
    fn __esp_wpa_sm_key_request(sm: *mut c_void, error: i32, pairwise: i32);
    fn __esp_wpa_sm_key_request_end();
    fn wpa_supplicant_stop_countermeasures(data: *mut c_void, user_ctx: *mut c_void);
    fn eloop_cancel_timeout(
        handler: EloopCallback,
        eloop_data: *mut c_void,
        user_data: *mut c_void,
    ) -> i32;
    fn eloop_register_timeout(
        seconds: u32,
        microseconds: u32,
        handler: EloopCallback,
        eloop_data: *mut c_void,
        user_data: *mut c_void,
    ) -> i32;
}

struct MichaelTimer(UnsafeCell<RawOsiTimer>);

impl MichaelTimer {
    const fn new() -> Self {
        Self(UnsafeCell::new(RawOsiTimer {
            next: ptr::null_mut(),
            expire: 0,
            period: 0,
            callback: None,
            argument: ptr::null_mut(),
        }))
    }
}

unsafe impl Sync for MichaelTimer {}

static MIC_TIMER: MichaelTimer = MichaelTimer::new();
static CALLBACK_INSTALLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MichaelInstallError {
    SupplicantNotInitialized,
    UnexpectedOriginalCallback(usize),
    UnexpectedKeyRequestSize(usize),
}

/// Replace the Michael MIC callback in the runtime-allocated WPA callback
/// table. Call this after `esp_supplicant_init` and before enabling STA RX.
///
/// The `wpa-async-mic` feature also requires the linker fragment
/// `ld/esp32s31-wpa-locals.x` so the local key-request helper can be called
/// without modifying the vendor archive.
///
/// # Safety
/// The registered S31 blob must match the digest verified by the workspace
/// audit. The caller must serialize this installation with supplicant init and
/// deinit.
pub unsafe fn install_async_michael_callback() -> Result<(), MichaelInstallError> {
    let key_request_size = (__esp_wpa_sm_key_request_end as *const () as usize)
        .wrapping_sub(__esp_wpa_sm_key_request as *const () as usize);
    if key_request_size != 0x146 {
        return Err(MichaelInstallError::UnexpectedKeyRequestSize(
            key_request_size,
        ));
    }

    let callbacks = ptr::addr_of!(wpa_cb).read();
    if callbacks.is_null() {
        return Err(MichaelInstallError::SupplicantNotInitialized);
    }

    let slot = callbacks
        .cast::<u8>()
        .add(MIC_CALLBACK_OFFSET)
        .cast::<MichaelCallback>();
    let current = slot.read() as usize;
    let replacement = async_michael_mic_failure as MichaelCallback;
    if current != wpa_michael_mic_failure as MichaelCallback as usize
        && current != replacement as usize
    {
        return Err(MichaelInstallError::UnexpectedOriginalCallback(current));
    }
    slot.write(replacement);
    CALLBACK_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// Restore the vendor callback and cancel a pending 10 ms continuation.
/// Call this before `esp_supplicant_deinit` releases `wpa_cb`.
///
/// # Safety
/// The caller must serialize this operation with callback invocation and
/// supplicant deinitialization.
pub unsafe fn uninstall_async_michael_callback() -> Result<(), MichaelInstallError> {
    let _ = cancel_internal_timer(MIC_TIMER.0.get().cast());
    let callbacks = ptr::addr_of!(wpa_cb).read();
    if callbacks.is_null() {
        CALLBACK_INSTALLED.store(false, Ordering::Release);
        return Err(MichaelInstallError::SupplicantNotInitialized);
    }

    let slot = callbacks
        .cast::<u8>()
        .add(MIC_CALLBACK_OFFSET)
        .cast::<MichaelCallback>();
    let current = slot.read() as usize;
    let replacement = async_michael_mic_failure as MichaelCallback;
    if current == replacement as usize {
        slot.write(wpa_michael_mic_failure);
    } else if current != wpa_michael_mic_failure as MichaelCallback as usize {
        return Err(MichaelInstallError::UnexpectedOriginalCallback(current));
    }
    CALLBACK_INSTALLED.store(false, Ordering::Release);
    Ok(())
}

pub fn async_michael_callback_installed() -> bool {
    CALLBACK_INSTALLED.load(Ordering::Acquire)
}

unsafe extern "C" fn async_michael_mic_failure(isunicast: u16) -> i32 {
    let state = ptr::addr_of_mut!(gWpaSm).cast::<u8>();
    let failures = state.add(MIC_FAILURES_OFFSET).cast::<u32>();

    if failures.read() == 0 {
        failures.write(1);
        wpa_sm_set_state(11);
        __esp_wpa_sm_key_request(state.cast(), 1, i32::from(isunicast));
        finish_countermeasure_timeout(ptr::null_mut());
        return 0;
    }

    wpa_sm_set_state(12);
    __esp_wpa_sm_key_request(state.cast(), 1, i32::from(isunicast));
    state.add(COUNTERMEASURES_OFFSET).cast::<u32>().write(1);

    if !schedule_internal_timer(
        MIC_TIMER.0.get().cast(),
        michael_settle_complete,
        ptr::null_mut(),
        MIC_SETTLE_DELAY_US,
    ) {
        // Failing open would leave countermeasures permanently enabled. The
        // finite timeout registration is safe to perform immediately.
        finish_countermeasure_timeout(ptr::null_mut());
    }
    0
}

unsafe extern "C" fn michael_settle_complete(argument: *mut c_void) {
    finish_countermeasure_timeout(argument);
}

unsafe fn finish_countermeasure_timeout(_argument: *mut c_void) {
    let callback = wpa_supplicant_stop_countermeasures as EloopCallback;
    eloop_cancel_timeout(callback, ptr::null_mut(), ptr::null_mut());
    eloop_register_timeout(
        COUNTERMEASURE_TIMEOUT_SECONDS,
        0,
        callback,
        ptr::null_mut(),
        ptr::null_mut(),
    );
}
