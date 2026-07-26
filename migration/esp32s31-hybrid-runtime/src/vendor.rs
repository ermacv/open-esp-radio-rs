use core::{ffi::c_void, ptr};

#[cfg(feature = "hil-vendor-tx")]
use core::sync::atomic::{AtomicUsize, Ordering};

use esp_wifi_sys_esp32s31::include::wifi_osi_funcs_t;

use crate::{
    context::RadioContextGuard,
    event::{PpAction, PpEvent},
    radio::{DispatchControl, PpDispatcher},
};

#[cfg(not(feature = "strict-no-wait"))]
type NoArgCallback = unsafe extern "C" fn();
type EventCallback = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    static mut g_osi_funcs_p: *const wifi_osi_funcs_t;
    static mut g_intr_lock_mux: *mut c_void;
    static mut pp_sig_cnt: [u8; 36];
    #[cfg(not(feature = "strict-no-wait"))]
    static mut g_net80211_tx_func: Option<NoArgCallback>;
    static mut g_config_func: Option<EventCallback>;
    #[cfg(not(feature = "strict-no-wait"))]
    static mut g_timer_func: Option<EventCallback>;

    #[cfg(not(feature = "strict-no-wait"))]
    fn ppProcessTxQ(queue: u8) -> i32;
    #[cfg(feature = "strict-no-wait")]
    fn ieee80211_ioctl_process(argument: *mut c_void) -> i32;
    #[cfg(not(feature = "strict-no-wait"))]
    fn pp_timer_do_process(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn pp_default_event_handler(kind: u32, argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn ppProcessRxPktHdr(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn ppProcTxDone(force: u32);
    #[cfg(not(feature = "strict-no-wait"))]
    fn ppRxPkt();
    #[cfg(not(feature = "strict-no-wait"))]
    fn ppResortTxAMPDU(queue: u8);
    #[cfg(not(feature = "strict-no-wait"))]
    fn lmacProcessTxTimeout();
    #[cfg(not(feature = "strict-no-wait"))]
    fn lmacProcessTxComplete();
    #[cfg(not(feature = "strict-no-wait"))]
    fn lmacProcessCollisions_task();
    #[cfg(not(feature = "strict-no-wait"))]
    fn wdevProcessRxSucDataAll();
    #[cfg(not(feature = "strict-no-wait"))]
    fn pm_on_tbtt(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn pm_on_tsf_timer(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn pm_on_beacon_rx(interface: u32, frame: u32, length: u32, from_task: u32);
    #[cfg(not(feature = "strict-no-wait"))]
    fn wifi_process_bsscolor_collision();
    #[cfg(not(feature = "strict-no-wait"))]
    fn pm_on_mac_modem_beacon_miss(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn wdevProcessModemStateRxBeacon(argument: *mut c_void);
    #[cfg(not(feature = "strict-no-wait"))]
    fn pm_on_coex_preemption_end(argument: *mut c_void);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VendorDispatchError {
    OsiNotRegistered,
    CriticalSectionCallbackMissing,
    SignalCounterUnderflow(u32),
    CallbackNotRegistered(PpAction),
    FatalEvent(usize),
    InternalQueueFull,
    RxExecutorUnavailable,
    #[cfg(feature = "strict-no-wait")]
    LmacContinuation(crate::lmac::LmacAsyncError),
    #[cfg(feature = "strict-no-wait")]
    TxQueue(crate::tx_queue::TxQueueProcessError),
    #[cfg(feature = "strict-no-wait")]
    UnsupportedStrictAction(PpAction),
    #[cfg(feature = "strict-no-wait")]
    UnsupportedStrictEvent(u32),
    #[cfg(feature = "strict-no-wait")]
    TxDoneContinuation(crate::txdone::TxDoneError),
    #[cfg(feature = "strict-no-wait")]
    FtmUnsupported,
    #[cfg(feature = "strict-no-wait")]
    WdevRxContinuation(crate::wdev::WdevRxContinuationError),
    #[cfg(feature = "strict-no-wait")]
    PromiscuousRxUnsupported,
    #[cfg(feature = "strict-no-wait")]
    UnexpectedInitializationConfigCallback(usize),
    #[cfg(feature = "strict-no-wait")]
    Net80211Timer(crate::net80211_timer::Net80211TimerError),
    #[cfg(feature = "strict-no-wait")]
    Net80211Tx(crate::net80211_tx::Net80211TxError),
    #[cfg(feature = "strict-no-wait")]
    RxPump(crate::rx::RxPumpError),
    #[cfg(feature = "strict-no-wait")]
    ChannelSwitch(crate::channel_switch::ChannelSwitchError),
    #[cfg(feature = "hil-ampdu-intercept")]
    TxAmpduIntercept(crate::tx_intercept::TxInterceptError),
}

/// Calls the original finite PP handlers selected by the recovered `ppTask`
/// jump table. The infinite `ppTask` function itself is never entered.
pub struct VendorPpDispatcher {
    allow_initialization_config: bool,
    rx_executor: Option<crate::adapter::RxExecutorCapability>,
}

/// Laboratory-only observation of the strict event-17 boundary.
///
/// A completed count smaller than the entered count means the vendor RX leaf
/// did not return. Equal counts prove only that the queued RX pump remained
/// finite; a separate ingress counter is needed to observe EAPOL delivery.
#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VendorRxDiagnosticSnapshot {
    pub entered: usize,
    pub completed: usize,
}

#[cfg(feature = "hil-vendor-tx")]
static RX_DISPATCH_ENTERED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static RX_DISPATCH_COMPLETED: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "hil-vendor-tx")]
static PP_TIMER_DISPATCH_ENTERED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static PP_TIMER_ID_MASK: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static PP_TIMER_NULL_ARGUMENTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static PP_TIMER_INVALID_IDS: AtomicUsize = AtomicUsize::new(0);

/// Read the event-17 counters without calling into the vendor library.
#[cfg(feature = "hil-vendor-tx")]
pub fn vendor_rx_diagnostic_snapshot() -> VendorRxDiagnosticSnapshot {
    VendorRxDiagnosticSnapshot {
        entered: RX_DISPATCH_ENTERED.load(Ordering::Acquire),
        completed: RX_DISPATCH_COMPLETED.load(Ordering::Acquire),
    }
}

/// Laboratory-only observation of PP event 8.
///
/// The recovered table has exactly sixteen IDs (`0..=15`). Recording only a
/// count and bitmap keeps the observation allocation-free and does not alter
/// the vendor callback/free ownership of the timer envelope.
#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpTimerDiagnosticSnapshot {
    pub entered: usize,
    pub id_mask: usize,
    pub null_arguments: usize,
    pub invalid_ids: usize,
}

#[cfg(feature = "hil-vendor-tx")]
pub fn pp_timer_diagnostic_snapshot() -> PpTimerDiagnosticSnapshot {
    PpTimerDiagnosticSnapshot {
        entered: PP_TIMER_DISPATCH_ENTERED.load(Ordering::Acquire),
        id_mask: PP_TIMER_ID_MASK.load(Ordering::Acquire),
        null_arguments: PP_TIMER_NULL_ARGUMENTS.load(Ordering::Acquire),
        invalid_ids: PP_TIMER_INVALID_IDS.load(Ordering::Acquire),
    }
}

#[cfg(feature = "hil-vendor-tx")]
unsafe fn observe_pp_timer(argument: *mut c_void) {
    PP_TIMER_DISPATCH_ENTERED.fetch_add(1, Ordering::Relaxed);
    let Some(argument) = argument.cast::<u8>().as_ref() else {
        PP_TIMER_NULL_ARGUMENTS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let id = *argument as usize;
    if id < 16 {
        PP_TIMER_ID_MASK.fetch_or(1 << id, Ordering::Release);
    } else {
        PP_TIMER_INVALID_IDS.fetch_add(1, Ordering::Release);
    }
}

impl VendorPpDispatcher {
    pub(crate) const fn new(rx_executor: crate::adapter::RxExecutorCapability) -> Self {
        Self {
            allow_initialization_config: false,
            rx_executor: Some(rx_executor),
        }
    }

    /// Construct the finite dispatcher used only by the cold-start drain.
    ///
    /// Event 6 is accepted only when the blob registered the pinned
    /// `ieee80211_ioctl_process` callback. The normal async dispatcher keeps
    /// rejecting event 6 because its envelope may carry synchronous API
    /// completion ownership.
    pub(crate) const fn for_initialization() -> Self {
        Self {
            allow_initialization_config: true,
            rx_executor: None,
        }
    }

    #[cfg(feature = "strict-no-wait")]
    fn dispatch_owned_rx(&mut self) -> Result<(), VendorDispatchError> {
        let executor = self
            .rx_executor
            .as_mut()
            .ok_or(VendorDispatchError::RxExecutorUnavailable)?;
        unsafe { crate::rx::dispatch(executor) }.map_err(VendorDispatchError::RxPump)
    }

    unsafe fn account_received_event(event: PpEvent) -> Result<(), VendorDispatchError> {
        if !event.has_signal_counter() {
            return Ok(());
        }

        let strict = crate::critical::strict_wifi_hart_armed();
        let (restore, mux, saved) = if strict {
            (
                None,
                ptr::null_mut(),
                crate::critical::strict_wifi_int_disable(),
            )
        } else {
            let osi = ptr::addr_of!(g_osi_funcs_p).read();
            let Some(osi) = osi.as_ref() else {
                return Err(VendorDispatchError::OsiNotRegistered);
            };
            let disable = osi
                ._wifi_int_disable
                .ok_or(VendorDispatchError::CriticalSectionCallbackMissing)?;
            let restore = osi
                ._wifi_int_restore
                .ok_or(VendorDispatchError::CriticalSectionCallbackMissing)?;
            let mux = ptr::addr_of!(g_intr_lock_mux).read();
            let saved = disable(mux);
            (Some(restore), mux, saved)
        };

        let counter = ptr::addr_of_mut!(pp_sig_cnt)
            .cast::<u8>()
            .add(event.kind as usize);
        let value = counter.read();
        if value == 0 {
            if let Some(restore) = restore {
                restore(mux, saved);
            } else {
                crate::critical::strict_wifi_int_restore(saved);
            }
            return Err(VendorDispatchError::SignalCounterUnderflow(event.kind));
        }
        counter.write(value.wrapping_sub(1));
        if let Some(restore) = restore {
            restore(mux, saved);
        } else {
            crate::critical::strict_wifi_int_restore(saved);
        }
        Ok(())
    }

    #[cfg(not(feature = "strict-no-wait"))]
    unsafe fn registered_no_arg(
        callback: *const Option<NoArgCallback>,
        action: PpAction,
    ) -> Result<(), VendorDispatchError> {
        let callback = callback
            .read()
            .ok_or(VendorDispatchError::CallbackNotRegistered(action))?;
        callback();
        Ok(())
    }

    #[cfg(not(feature = "strict-no-wait"))]
    unsafe fn optional_event(callback: *const Option<EventCallback>, argument: *mut c_void) {
        if let Some(callback) = callback.read() {
            callback(argument);
        }
    }
}

impl PpDispatcher for VendorPpDispatcher {
    type Error = VendorDispatchError;

    fn dispatch(&mut self, event: PpEvent) -> Result<DispatchControl, Self::Error> {
        let _context = RadioContextGuard::enter(event.kind);
        unsafe {
            #[cfg(feature = "strict-no-wait")]
            if crate::lmac::txq_split_failed() {
                return Err(VendorDispatchError::LmacContinuation(
                    crate::lmac::LmacAsyncError::TxQueueSplitFailed,
                ));
            }

            #[cfg(feature = "strict-no-wait")]
            if let Some(error) = crate::channel_switch::failure() {
                return Err(VendorDispatchError::ChannelSwitch(error));
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::net80211_tx::is_power_save_continuation(event.kind) {
                crate::net80211_tx::dispatch_power_save_continuation(event.argument)
                    .map_err(VendorDispatchError::Net80211Tx)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::lmac::is_continuation(event.kind) {
                crate::lmac::dispatch_continuation()
                    .map_err(VendorDispatchError::LmacContinuation)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::txdone::is_continuation(event.kind) {
                crate::txdone::dispatch_continuation()
                    .map_err(VendorDispatchError::TxDoneContinuation)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::txdone::is_lmac_continuation(event.kind) {
                crate::txdone::dispatch_lmac_continuation()
                    .map_err(VendorDispatchError::TxDoneContinuation)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::lmac::is_ampdu_completion_continuation(event.kind) {
                crate::lmac::dispatch_ampdu_completion()
                    .map_err(VendorDispatchError::LmacContinuation)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "hil-ampdu-intercept")]
            if crate::tx_intercept::is_event(event.kind) {
                crate::tx_intercept::dispatch().map_err(VendorDispatchError::TxAmpduIntercept)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if crate::rx::is_continuation(event.kind) {
                self.dispatch_owned_rx()?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if event.kind == crate::scan::SCAN_CHANNEL_EVENT {
                crate::scan::dispatch_channel();
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if event.kind == crate::sta_link::STA_AUTH_EVENT {
                crate::sta_link::dispatch_auth_tx();
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if event.kind == crate::sta_link::STA_ASSOC_EVENT {
                crate::sta_link::dispatch_assoc_tx();
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "strict-no-wait")]
            if event.kind == crate::net80211_timer::NET80211_TIMER_EVENT {
                crate::net80211_timer::dispatch(event.argument)
                    .map_err(VendorDispatchError::Net80211Timer)?;
                return Ok(DispatchControl::Continue);
            }

            #[cfg(feature = "wpa-async-eap")]
            if crate::eap::is_async_work(event.kind) {
                let follow_up = match crate::eap::dispatch(event.kind) {
                    crate::eap::DispatchResult::Complete => None,
                    crate::eap::DispatchResult::Deferred => Some(event.kind),
                    crate::eap::DispatchResult::ContinueRx => Some(crate::eap::EAP_RX_CONTINUATION),
                };
                if let Some(kind) = follow_up {
                    if !crate::adapter::enqueue_internal_event(PpEvent {
                        kind,
                        argument: ptr::null_mut(),
                    }) {
                        return Err(VendorDispatchError::InternalQueueFull);
                    }
                }
                return Ok(DispatchControl::Continue);
            }

            Self::account_received_event(event)?;

            match event.action() {
                PpAction::ProcessTxQueue(queue) => {
                    #[cfg(feature = "strict-no-wait")]
                    crate::tx_queue::process_tx_queue(queue)
                        .map_err(VendorDispatchError::TxQueue)?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    ppProcessTxQ(queue);
                }
                PpAction::Net80211Tx => {
                    #[cfg(feature = "strict-no-wait")]
                    {
                        crate::net80211_tx::dispatch_one()
                            .map_err(VendorDispatchError::Net80211Tx)?;
                    }
                    #[cfg(not(feature = "strict-no-wait"))]
                    Self::registered_no_arg(
                        ptr::addr_of!(g_net80211_tx_func),
                        PpAction::Net80211Tx,
                    )?;
                }
                // The original dispatcher explicitly skips null config and
                // timer callbacks. The net80211 TX callback is not optional.
                PpAction::Config => {
                    #[cfg(feature = "strict-no-wait")]
                    {
                        if !self.allow_initialization_config {
                            return Err(VendorDispatchError::UnsupportedStrictAction(
                                PpAction::Config,
                            ));
                        }
                        let callback = ptr::addr_of!(g_config_func)
                            .read()
                            .ok_or(VendorDispatchError::CallbackNotRegistered(PpAction::Config))?;
                        if callback as *const () as usize
                            != ieee80211_ioctl_process as *const () as usize
                        {
                            return Err(
                                VendorDispatchError::UnexpectedInitializationConfigCallback(
                                    callback as *const () as usize,
                                ),
                            );
                        }
                        let _ = ieee80211_ioctl_process(event.argument);
                    }
                    #[cfg(not(feature = "strict-no-wait"))]
                    Self::optional_event(ptr::addr_of!(g_config_func), event.argument)
                }
                PpAction::TimerCallback => {
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::UnsupportedStrictAction(
                        PpAction::TimerCallback,
                    ));
                    #[cfg(not(feature = "strict-no-wait"))]
                    Self::optional_event(ptr::addr_of!(g_timer_func), event.argument)
                }
                PpAction::PpTimer => {
                    #[cfg(feature = "hil-vendor-tx")]
                    observe_pp_timer(event.argument);
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::UnsupportedStrictAction(
                        PpAction::PpTimer,
                    ));
                    #[cfg(not(feature = "strict-no-wait"))]
                    pp_timer_do_process(event.argument)
                }
                PpAction::Default => {
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::UnsupportedStrictEvent(event.kind));
                    #[cfg(not(feature = "strict-no-wait"))]
                    pp_default_event_handler(event.kind, event.argument)
                }
                PpAction::ProcessRxHeader => {
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::PromiscuousRxUnsupported);
                    #[cfg(not(feature = "strict-no-wait"))]
                    ppProcessRxPktHdr(event.argument)
                }
                PpAction::Fatal => {
                    return Err(VendorDispatchError::FatalEvent(event.argument as usize));
                }
                PpAction::Shutdown => {
                    crate::adapter::mark_shutdown_processed();
                    return Ok(DispatchControl::Stop);
                }
                PpAction::ProcessTxDone => {
                    #[cfg(feature = "strict-no-wait")]
                    crate::txdone::begin().map_err(VendorDispatchError::TxDoneContinuation)?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    ppProcTxDone(1);
                }
                PpAction::ProcessRxPacket => {
                    #[cfg(feature = "hil-vendor-tx")]
                    RX_DISPATCH_ENTERED.fetch_add(1, Ordering::Relaxed);
                    #[cfg(feature = "strict-no-wait")]
                    self.dispatch_owned_rx()?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    {
                        let _executor = self
                            .rx_executor
                            .as_mut()
                            .ok_or(VendorDispatchError::RxExecutorUnavailable)?;
                        ppRxPkt();
                    }
                    #[cfg(feature = "hil-vendor-tx")]
                    RX_DISPATCH_COMPLETED.fetch_add(1, Ordering::Release);
                }
                PpAction::ResortTxAmpdu => {
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::UnsupportedStrictAction(
                        PpAction::ResortTxAmpdu,
                    ));
                    #[cfg(not(feature = "strict-no-wait"))]
                    ppResortTxAMPDU(event.argument as usize as u8);
                }
                PpAction::Noop => {}
                PpAction::LmacTxTimeout => {
                    #[cfg(feature = "strict-no-wait")]
                    crate::lmac::begin_tx_timeout()
                        .map_err(VendorDispatchError::LmacContinuation)?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    lmacProcessTxTimeout();
                }
                PpAction::LmacTxComplete => {
                    #[cfg(feature = "strict-no-wait")]
                    crate::lmac::process_tx_complete()
                        .map_err(VendorDispatchError::LmacContinuation)?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    lmacProcessTxComplete();
                }
                PpAction::LmacCollision => {
                    #[cfg(feature = "strict-no-wait")]
                    crate::lmac::process_tx_collision()
                        .map_err(VendorDispatchError::LmacContinuation)?;
                    #[cfg(not(feature = "strict-no-wait"))]
                    lmacProcessCollisions_task();
                }
                PpAction::WdevRxSuccess => {
                    #[cfg(feature = "strict-no-wait")]
                    {
                        // RX completion itself is a stronger wake source than
                        // the fallback reload timer.  Commit any MAC-accepted
                        // recycle tail before the decoder can detach and
                        // append descriptors using `wDevCtrl.tail`.
                        crate::wdev::settle_rx_reload_before_success();
                        crate::wdev::process_rx_success()
                            .map_err(VendorDispatchError::WdevRxContinuation)?;
                    }
                    #[cfg(not(feature = "strict-no-wait"))]
                    wdevProcessRxSucDataAll();
                    #[cfg(feature = "strict-no-wait")]
                    if crate::wdev::take_ftm_attempted() {
                        return Err(VendorDispatchError::FtmUnsupported);
                    }
                }
                action @ (PpAction::PowerSaveTbtt
                | PpAction::PowerSaveTsfTimer
                | PpAction::PowerSaveBeaconRx
                | PpAction::PowerSaveBeaconMiss
                | PpAction::WdevModemStateRxBeacon
                | PpAction::CoexPreemptionEnd) => {
                    #[cfg(feature = "strict-no-wait")]
                    return Err(VendorDispatchError::UnsupportedStrictAction(action));
                    #[cfg(not(feature = "strict-no-wait"))]
                    match action {
                        PpAction::PowerSaveTbtt => pm_on_tbtt(event.argument),
                        PpAction::PowerSaveTsfTimer => pm_on_tsf_timer(event.argument),
                        PpAction::PowerSaveBeaconRx => pm_on_beacon_rx(0, 0, 0, 1),
                        PpAction::BssColorCollision => wifi_process_bsscolor_collision(),
                        PpAction::PowerSaveBeaconMiss => {
                            pm_on_mac_modem_beacon_miss(event.argument)
                        }
                        PpAction::WdevModemStateRxBeacon => {
                            wdevProcessModemStateRxBeacon(event.argument)
                        }
                        PpAction::CoexPreemptionEnd => pm_on_coex_preemption_end(event.argument),
                        _ => unreachable!(),
                    }
                }
                PpAction::BssColorCollision => {
                    #[cfg(all(
                        feature = "strict-no-wait",
                        feature = "hil-he-association-oracle"
                    ))]
                    if !unsafe { crate::he::consume_disabled_bss_color_collision() } {
                        return Err(VendorDispatchError::UnsupportedStrictAction(
                            PpAction::BssColorCollision,
                        ));
                    }
                    #[cfg(all(
                        feature = "strict-no-wait",
                        not(feature = "hil-he-association-oracle")
                    ))]
                    return Err(VendorDispatchError::UnsupportedStrictAction(
                        PpAction::BssColorCollision,
                    ));
                    #[cfg(not(feature = "strict-no-wait"))]
                    wifi_process_bsscolor_collision();
                }
            }
        }

        Ok(DispatchControl::Continue)
    }
}
