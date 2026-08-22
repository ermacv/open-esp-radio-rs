//! Embassy time binding for the portable WPA2 station transaction runner.
//!
//! Protocol state, deadlines and rollback ordering live in
//! `open-esp-radio-wpa2`. This adapter contributes only the Embassy monotonic
//! clock used by ESP32-S31 firmware.

use core::future::Future;

use embassy_time::{Instant, Timer};

use open_esp_radio_wpa2::runner::Wpa2HandshakeTimer;

#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyWpa2HandshakeTimer;

impl Wpa2HandshakeTimer for EmbassyWpa2HandshakeTimer {
    fn now_micros(&self) -> u64 {
        Instant::now().as_micros()
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::at(Instant::from_micros(deadline_micros))
    }
}
