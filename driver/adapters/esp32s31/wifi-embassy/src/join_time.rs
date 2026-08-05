//! Embassy time binding for the portable STA join transaction.

use core::future::Future;

use embassy_time::{Instant, Timer};

use open_esp_radio_wifi_sta::join::StaJoinTimer;

/// Production Embassy-time adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyStaJoinTimer;

impl StaJoinTimer for EmbassyStaJoinTimer {
    fn now_micros(&self) -> u64 {
        Instant::now().as_micros()
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::at(Instant::from_micros(deadline_micros))
    }
}
