//! Executor-independent platform ports and resources for ESP32-S31 STA TX.
//!
//! The ordinary TX owner may await these ports, but this module does not pick
//! an executor, timer driver or entropy peripheral. It also translates the
//! owned PHY calibration profile into the narrow power lookup consumed by the
//! Wi-Fi descriptor path without exposing the vendor parameter image.

use core::{future::Future, pin::Pin};

use open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile;
use open_esp_radio_esp32s31_wifi_mac::{tx::TxSlot, tx_runtime::StaTxRuntimePolicy};

/// State of one finite ESP32-S31 STA TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxProgress {
    /// DMA, acknowledgement or a bounded retry is still in flight.
    Pending,
    /// Hardware no longer owns the TX descriptor or its frame.
    Complete,
}

/// Reason for inspecting one active TX transaction.
///
/// The executor decides how either edge is produced. The transaction owner
/// consumes only this value and therefore remains independent of Embassy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxWake {
    /// A coalesced completion, hardware-timeout or collision interrupt fired.
    Interrupt { events: u32 },
    /// The transaction's external deadline expired without a decisive IRQ.
    Deadline,
}

/// Platform entropy input used only for bounded EDCA slot selection.
pub trait WifiTxEntropy {
    fn next_u32(&mut self) -> u32;
}

impl<F: FnMut() -> u32> WifiTxEntropy for F {
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

/// Calibrated MAC power lookup without exposing a vendor parameter image.
pub trait WifiTxPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl WifiTxPowerProfile for PhyTxTargetPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        let pair = self.pair(rate_code);
        WifiTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

/// Monotonic time and the two bounded asynchronous edges used by STA TX.
pub trait WifiTxTimer {
    fn now_micros(&self) -> u64;
    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;
    fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_;
}

/// Finite policy for management and EAPOL frames sent before connected IRQ
/// scheduling owns the ordinary descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlTxConfig {
    /// Maximum hardware publications for one unicast MPDU.
    pub unicast_attempt_limit: u8,
    /// Executor watchdog for each hardware publication.
    pub completion_timeout_us: u64,
    /// Cooperative polling interval before the MAC IRQ owner is installed.
    pub poll_interval_us: u64,
}

/// Resources whose ownership must stay together for every ordinary TX phase.
///
/// Protocol owners add their own finite configuration instead of exposing a
/// growing positional constructor whenever another runtime or hardware port
/// is introduced.
pub struct WifiTxResources<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    pub policy: StaTxRuntimePolicy,
    pub power: P,
    pub entropy: E,
    pub timer: T,
}
