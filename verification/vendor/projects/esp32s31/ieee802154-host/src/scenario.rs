//! Scenario steps executed against the compiled vendor driver.
//!
//! A step is either one public ESP-IDF call or one interrupt delivered through
//! the handler the driver registered with `esp_intr_alloc`. The compiled
//! driver keeps process-global state, so one process runs one scenario; the
//! [`crate::claim`] guard enforces that.

use core::ffi::c_void;

use crate::record::{self, Inputs, Record};

/// Portable driver call or hardware edge.
#[derive(Clone, Debug)]
pub enum Step {
    /// `esp_ieee802154_enable`.
    Enable,
    /// `esp_ieee802154_disable`.
    Disable,
    /// `esp_ieee802154_set_channel`.
    SetChannel(u8),
    /// `esp_ieee802154_set_panid`.
    SetPanId(u16),
    /// `esp_ieee802154_set_short_address`.
    SetShortAddress(u16),
    /// `esp_ieee802154_set_extended_address`.
    SetExtendedAddress([u8; 8]),
    /// `esp_ieee802154_set_promiscuous`.
    SetPromiscuous(bool),
    /// `esp_ieee802154_set_coordinator`.
    SetCoordinator(bool),
    /// `esp_ieee802154_set_rx_when_idle`.
    SetRxWhenIdle(bool),
    /// `esp_ieee802154_set_cca_mode`.
    SetCcaMode(u32),
    /// `esp_ieee802154_set_cca_threshold`.
    SetCcaThreshold(i8),
    /// `esp_ieee802154_set_ack_timeout` in microseconds.
    SetAckTimeout(u32),
    /// `esp_ieee802154_get_panid`, `get_short_address`,
    /// `get_extended_address` and `get_ack_timeout`, in that order.
    GetIdentity,
    /// `esp_ieee802154_set_transmit_security` for `[length, psdu...]`.
    SetTransmitSecurity {
        frame: Vec<u8>,
        key: [u8; 16],
        address: [u8; 8],
    },
    /// `esp_ieee802154_set_pending_mode`.
    SetPendingMode(u32),
    /// `esp_ieee802154_add_pending_addr`.
    AddPendingAddress { address: Vec<u8>, short: bool },
    /// `esp_ieee802154_transmit` of `[length, psdu...]`.
    Transmit { frame: Vec<u8>, cca: bool },
    /// `esp_ieee802154_transmit_at` of `[length, psdu...]` at `time`.
    TransmitAt {
        frame: Vec<u8>,
        cca: bool,
        time: u32,
    },
    /// `esp_ieee802154_receive`.
    Receive,
    /// `esp_ieee802154_receive_at`.
    ReceiveAt { time: u32, duration: u32 },
    /// `esp_ieee802154_sleep`.
    Sleep,
    /// `esp_ieee802154_energy_detect` in 16-microsecond symbols.
    EnergyDetect(u32),
    /// `esp_ieee802154_cca`.
    Cca,
    /// `esp_ieee802154_receive_handle_done` for the last received frame.
    ReceiveHandleDone,
    /// Set the value a named getter or external call returns from now on.
    Input(&'static str, u64),
    /// Write `[length, psdu...]` into the RX buffer the driver last published.
    DeliverFrame(Vec<u8>),
    /// Latch an event image and abort reasons, then run the interrupt handler.
    Interrupt {
        events: u16,
        rx_abort: u8,
        tx_abort: u8,
    },
}

/// Complete scenario: vendor inputs and ordered steps.
pub struct Scenario {
    pub name: &'static str,
    pub inputs: fn() -> Inputs,
    pub steps: fn() -> Vec<Step>,
}

unsafe extern "C" {
    fn esp_ieee802154_enable() -> i32;
    fn esp_ieee802154_disable() -> i32;
    fn esp_ieee802154_set_channel(channel: u8) -> i32;
    fn esp_ieee802154_set_panid(panid: u16) -> i32;
    fn esp_ieee802154_set_short_address(address: u16) -> i32;
    fn esp_ieee802154_set_extended_address(address: *const u8) -> i32;
    fn esp_ieee802154_set_promiscuous(enable: bool) -> i32;
    fn esp_ieee802154_set_coordinator(enable: bool) -> i32;
    fn esp_ieee802154_set_rx_when_idle(enable: bool) -> i32;
    fn esp_ieee802154_set_cca_mode(mode: u32) -> i32;
    fn esp_ieee802154_set_cca_threshold(threshold: i8) -> i32;
    fn esp_ieee802154_set_ack_timeout(timeout: u32) -> i32;
    fn esp_ieee802154_get_panid() -> u16;
    fn esp_ieee802154_get_short_address() -> u16;
    fn esp_ieee802154_get_extended_address(address: *mut u8) -> i32;
    fn esp_ieee802154_get_ack_timeout() -> u32;
    fn esp_ieee802154_set_transmit_security(frame: *mut u8, key: *mut u8, address: *mut u8) -> i32;
    fn esp_ieee802154_set_pending_mode(mode: u32) -> i32;
    fn esp_ieee802154_add_pending_addr(address: *const u8, short: bool) -> i32;
    fn esp_ieee802154_transmit(frame: *const u8, cca: bool) -> i32;
    fn esp_ieee802154_transmit_at(frame: *const u8, cca: bool, time: u32) -> i32;
    fn esp_ieee802154_receive() -> i32;
    fn esp_ieee802154_receive_at(time: u32, duration: u32) -> i32;
    fn esp_ieee802154_sleep() -> i32;
    fn esp_ieee802154_energy_detect(duration: u32) -> i32;
    fn esp_ieee802154_cca() -> i32;
    fn esp_ieee802154_receive_handle_done(frame: *const u8) -> i32;
}

/// Pinned vendor event, abort and snapshot inputs read by the interrupt handler.
const EVENTS: &str = "ieee802154_ll_get_events";
const RX_ABORT: &str = "ieee802154_ll_get_rx_abort_reason";
const TX_ABORT: &str = "ieee802154_ll_get_tx_abort_reason";

/// Outcome of one public call, recorded as an external boundary value.
fn returned(name: &str, value: i32) {
    record::record_return(name, value);
}

fn run_step(step: Step, last_rx: &mut Option<usize>) {
    // SAFETY (all calls below): the compiled vendor driver is linked into this
    // process, the claim guard serialises it, and every pointer argument
    // outlives the call. Transmitted frames are leaked because the driver
    // keeps the frame pointer until its terminal callback.
    unsafe {
        match step {
            Step::Enable => returned("esp_ieee802154_enable", esp_ieee802154_enable()),
            Step::Disable => returned("esp_ieee802154_disable", esp_ieee802154_disable()),
            Step::SetChannel(channel) => returned(
                "esp_ieee802154_set_channel",
                esp_ieee802154_set_channel(channel),
            ),
            Step::SetPanId(panid) => {
                returned("esp_ieee802154_set_panid", esp_ieee802154_set_panid(panid))
            }
            Step::SetShortAddress(address) => returned(
                "esp_ieee802154_set_short_address",
                esp_ieee802154_set_short_address(address),
            ),
            Step::SetExtendedAddress(address) => returned(
                "esp_ieee802154_set_extended_address",
                esp_ieee802154_set_extended_address(address.as_ptr()),
            ),
            Step::SetPromiscuous(enable) => returned(
                "esp_ieee802154_set_promiscuous",
                esp_ieee802154_set_promiscuous(enable),
            ),
            Step::SetCoordinator(enable) => returned(
                "esp_ieee802154_set_coordinator",
                esp_ieee802154_set_coordinator(enable),
            ),
            Step::SetRxWhenIdle(enable) => returned(
                "esp_ieee802154_set_rx_when_idle",
                esp_ieee802154_set_rx_when_idle(enable),
            ),
            Step::SetCcaMode(mode) => returned(
                "esp_ieee802154_set_cca_mode",
                esp_ieee802154_set_cca_mode(mode),
            ),
            Step::SetCcaThreshold(threshold) => returned(
                "esp_ieee802154_set_cca_threshold",
                esp_ieee802154_set_cca_threshold(threshold),
            ),
            Step::SetAckTimeout(timeout) => returned(
                "esp_ieee802154_set_ack_timeout",
                esp_ieee802154_set_ack_timeout(timeout),
            ),
            Step::GetIdentity => {
                esp_ieee802154_get_panid();
                esp_ieee802154_get_short_address();
                let mut address = [0u8; 8];
                esp_ieee802154_get_extended_address(address.as_mut_ptr());
                esp_ieee802154_get_ack_timeout();
            }
            Step::SetTransmitSecurity {
                mut frame,
                mut key,
                mut address,
            } => returned(
                "esp_ieee802154_set_transmit_security",
                esp_ieee802154_set_transmit_security(
                    frame.as_mut_ptr(),
                    key.as_mut_ptr(),
                    address.as_mut_ptr(),
                ),
            ),
            Step::SetPendingMode(mode) => returned(
                "esp_ieee802154_set_pending_mode",
                esp_ieee802154_set_pending_mode(mode),
            ),
            Step::AddPendingAddress { address, short } => returned(
                "esp_ieee802154_add_pending_addr",
                esp_ieee802154_add_pending_addr(address.as_ptr(), short),
            ),
            Step::Transmit { frame, cca } => {
                let frame: &'static [u8] = Vec::leak(frame);
                record::register_transmit_frame(frame.as_ptr() as usize);
                returned(
                    "esp_ieee802154_transmit",
                    esp_ieee802154_transmit(frame.as_ptr(), cca),
                )
            }
            Step::TransmitAt { frame, cca, time } => {
                let frame: &'static [u8] = Vec::leak(frame);
                record::register_transmit_frame(frame.as_ptr() as usize);
                returned(
                    "esp_ieee802154_transmit_at",
                    esp_ieee802154_transmit_at(frame.as_ptr(), cca, time),
                )
            }
            Step::Receive => returned("esp_ieee802154_receive", esp_ieee802154_receive()),
            Step::ReceiveAt { time, duration } => returned(
                "esp_ieee802154_receive_at",
                esp_ieee802154_receive_at(time, duration),
            ),
            Step::Sleep => returned("esp_ieee802154_sleep", esp_ieee802154_sleep()),
            Step::EnergyDetect(duration) => returned(
                "esp_ieee802154_energy_detect",
                esp_ieee802154_energy_detect(duration),
            ),
            Step::Cca => returned("esp_ieee802154_cca", esp_ieee802154_cca()),
            Step::ReceiveHandleDone => {
                let frame = last_rx.expect("a frame must be delivered before it is released");
                returned(
                    "esp_ieee802154_receive_handle_done",
                    esp_ieee802154_receive_handle_done(frame as *const u8),
                )
            }
            Step::Input(name, value) => record::set_input(name, value),
            Step::DeliverFrame(frame) => {
                let address = record::rx_address()
                    .expect("the driver must publish an RX buffer before a frame arrives");
                assert!(frame.len() <= 128, "an RX buffer holds at most 128 bytes");
                std::ptr::copy_nonoverlapping(frame.as_ptr(), address as *mut u8, frame.len());
                *last_rx = Some(address);
            }
            Step::Interrupt {
                events,
                rx_abort,
                tx_abort,
            } => {
                record::set_input(EVENTS, u64::from(events));
                record::set_input(RX_ABORT, u64::from(rx_abort));
                record::set_input(TX_ABORT, u64::from(tx_abort));
                let (handler, argument) = record::interrupt_handler()
                    .expect("the driver must allocate its interrupt before one is delivered");
                let handler = core::mem::transmute::<usize, extern "C" fn(*mut c_void)>(handler);
                handler(argument as *mut c_void);
            }
        }
    }
}

/// Run one scenario and return its complete boundary trace.
pub fn run(scenario: &Scenario) -> Vec<Record> {
    record::reset((scenario.inputs)());
    let mut last_rx = None;
    for step in (scenario.steps)() {
        run_step(step, &mut last_rx);
    }
    record::take_records()
}
