use core::ffi::c_void;

/// Message consumed by the vendor `ppTask` queue on ESP32-S31.
///
/// The layout is recovered from `libpp.a(pp.o)`: `ppTask` receives eight-byte
/// queue items and reads the event number at offset 0 and its argument at
/// offset 4. ESP32-S31 is a 32-bit target.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpEvent {
    pub kind: u32,
    pub argument: *mut c_void,
}

// The pointer is opaque and is only consumed by the radio future. Ownership
// remains governed by the vendor event ABI.
unsafe impl Send for PpEvent {}

/// Operation selected by the jump table in ESP32-S31 `ppTask`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PpAction {
    ProcessTxQueue(u8),
    Net80211Tx,
    Config,
    TimerCallback,
    PpTimer,
    Default,
    ProcessRxHeader,
    Fatal,
    Shutdown,
    ProcessTxDone,
    ProcessRxPacket,
    ResortTxAmpdu,
    Noop,
    LmacTxTimeout,
    LmacTxComplete,
    LmacCollision,
    WdevRxSuccess,
    PowerSaveTbtt,
    PowerSaveTsfTimer,
    PowerSaveBeaconRx,
    BssColorCollision,
    PowerSaveBeaconMiss,
    WdevModemStateRxBeacon,
    CoexPreemptionEnd,
}

impl PpEvent {
    /// Decode the exact 34-entry jump table used by the pinned ESP32-S31 blob.
    /// Events outside that table go to `pp_default_event_handler`.
    pub const fn action(self) -> PpAction {
        match self.kind {
            0..=4 => PpAction::ProcessTxQueue(self.kind as u8),
            5 => PpAction::Net80211Tx,
            6 => PpAction::Config,
            7 => PpAction::TimerCallback,
            8 => PpAction::PpTimer,
            9..=12 => PpAction::Default,
            13 => PpAction::ProcessRxHeader,
            14 => PpAction::Fatal,
            15 => PpAction::Shutdown,
            16 => PpAction::ProcessTxDone,
            17 => PpAction::ProcessRxPacket,
            18 => PpAction::ResortTxAmpdu,
            19..=21 => PpAction::Noop,
            22 => PpAction::LmacTxTimeout,
            23 => PpAction::LmacTxComplete,
            24 => PpAction::LmacCollision,
            25 => PpAction::WdevRxSuccess,
            26 => PpAction::PowerSaveTbtt,
            27 => PpAction::PowerSaveTsfTimer,
            28 => PpAction::Default,
            29 => PpAction::PowerSaveBeaconRx,
            30 => PpAction::BssColorCollision,
            31 => PpAction::PowerSaveBeaconMiss,
            32 => PpAction::WdevModemStateRxBeacon,
            33 => PpAction::CoexPreemptionEnd,
            _ => PpAction::Default,
        }
    }

    /// Whether `ppTask` decrements `pp_sig_cnt[kind]` after receiving this
    /// event. Event 13 is deliberately not counted by the vendor `pp_post`.
    pub const fn has_signal_counter(self) -> bool {
        self.kind <= 35 && self.kind != 13
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<PpEvent>() == 8);

#[cfg(test)]
mod tests {
    use super::{PpAction, PpEvent};

    fn event(kind: u32) -> PpEvent {
        PpEvent {
            kind,
            argument: core::ptr::null_mut(),
        }
    }

    #[test]
    fn recovered_jump_table_is_stable() {
        assert_eq!(event(0).action(), PpAction::ProcessTxQueue(0));
        assert_eq!(event(4).action(), PpAction::ProcessTxQueue(4));
        assert_eq!(event(5).action(), PpAction::Net80211Tx);
        assert_eq!(event(8).action(), PpAction::PpTimer);
        assert_eq!(event(13).action(), PpAction::ProcessRxHeader);
        assert_eq!(event(15).action(), PpAction::Shutdown);
        assert_eq!(event(22).action(), PpAction::LmacTxTimeout);
        assert_eq!(event(33).action(), PpAction::CoexPreemptionEnd);
        assert_eq!(event(34).action(), PpAction::Default);
    }

    #[test]
    fn signal_counter_exception_matches_pp_post() {
        assert!(event(0).has_signal_counter());
        assert!(!event(13).has_signal_counter());
        assert!(event(35).has_signal_counter());
        assert!(!event(36).has_signal_counter());
    }
}
