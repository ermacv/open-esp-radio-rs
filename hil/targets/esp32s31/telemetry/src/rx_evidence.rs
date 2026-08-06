//! RX PHY-format and aggregation evidence accumulated by HIL.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_wifi_softmac::MacRxEvidence;

pub const RX_HE_MCS_BUCKETS: usize = 12;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxPhySnapshot {
    pub he_mcs: [u32; RX_HE_MCS_BUCKETS],
    pub other: u32,
}

pub struct RxPhyCounters {
    he_mcs: [AtomicU32; RX_HE_MCS_BUCKETS],
    other: AtomicU32,
}

impl RxPhyCounters {
    pub const fn new() -> Self {
        Self {
            he_mcs: [const { AtomicU32::new(0) }; RX_HE_MCS_BUCKETS],
            other: AtomicU32::new(0),
        }
    }

    pub fn observe_he_mcs(&self, mcs: u8) {
        match self.he_mcs.get(usize::from(mcs)) {
            Some(counter) => {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            None => self.observe_other(),
        }
    }

    pub fn observe_other(&self) {
        self.other.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RxPhySnapshot {
        RxPhySnapshot {
            he_mcs: core::array::from_fn(|index| self.he_mcs[index].load(Ordering::Relaxed)),
            other: self.other.load(Ordering::Relaxed),
        }
    }
}

impl Default for RxPhyCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxSmpduSnapshot {
    pub s_mpdu_frames: u32,
    pub not_s_mpdu_frames: u32,
    pub unavailable_frames: u32,
}

impl RxSmpduSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            s_mpdu_frames: self.s_mpdu_frames.wrapping_sub(earlier.s_mpdu_frames),
            not_s_mpdu_frames: self
                .not_s_mpdu_frames
                .wrapping_sub(earlier.not_s_mpdu_frames),
            unavailable_frames: self
                .unavailable_frames
                .wrapping_sub(earlier.unavailable_frames),
        }
    }
}

pub struct RxSmpduCounters {
    s_mpdu_frames: AtomicU32,
    not_s_mpdu_frames: AtomicU32,
    unavailable_frames: AtomicU32,
}

impl RxSmpduCounters {
    pub const fn new() -> Self {
        Self {
            s_mpdu_frames: AtomicU32::new(0),
            not_s_mpdu_frames: AtomicU32::new(0),
            unavailable_frames: AtomicU32::new(0),
        }
    }

    pub fn observe(&self, evidence: MacRxEvidence<bool>) {
        let counter = match evidence {
            MacRxEvidence::HardwareObserved(true) => &self.s_mpdu_frames,
            MacRxEvidence::HardwareObserved(false) => &self.not_s_mpdu_frames,
            MacRxEvidence::ProtocolValidated(_) | MacRxEvidence::Unavailable => {
                &self.unavailable_frames
            }
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RxSmpduSnapshot {
        RxSmpduSnapshot {
            s_mpdu_frames: self.s_mpdu_frames.load(Ordering::Relaxed),
            not_s_mpdu_frames: self.not_s_mpdu_frames.load(Ordering::Relaxed),
            unavailable_frames: self.unavailable_frames.load(Ordering::Relaxed),
        }
    }
}

impl Default for RxSmpduCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxAmpduSnapshot {
    pub ampdu_frames: u32,
    pub not_ampdu_frames: u32,
    pub hardware_ampdu_frames: u32,
    pub hardware_not_ampdu_frames: u32,
    pub protocol_ampdu_frames: u32,
    pub protocol_not_ampdu_frames: u32,
    pub unavailable_frames: u32,
}

impl RxAmpduSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            ampdu_frames: self.ampdu_frames.wrapping_sub(earlier.ampdu_frames),
            not_ampdu_frames: self.not_ampdu_frames.wrapping_sub(earlier.not_ampdu_frames),
            hardware_ampdu_frames: self
                .hardware_ampdu_frames
                .wrapping_sub(earlier.hardware_ampdu_frames),
            hardware_not_ampdu_frames: self
                .hardware_not_ampdu_frames
                .wrapping_sub(earlier.hardware_not_ampdu_frames),
            protocol_ampdu_frames: self
                .protocol_ampdu_frames
                .wrapping_sub(earlier.protocol_ampdu_frames),
            protocol_not_ampdu_frames: self
                .protocol_not_ampdu_frames
                .wrapping_sub(earlier.protocol_not_ampdu_frames),
            unavailable_frames: self
                .unavailable_frames
                .wrapping_sub(earlier.unavailable_frames),
        }
    }
}

pub struct RxAmpduCounters {
    ampdu_frames: AtomicU32,
    not_ampdu_frames: AtomicU32,
    hardware_ampdu_frames: AtomicU32,
    hardware_not_ampdu_frames: AtomicU32,
    protocol_ampdu_frames: AtomicU32,
    protocol_not_ampdu_frames: AtomicU32,
    unavailable_frames: AtomicU32,
}

impl RxAmpduCounters {
    pub const fn new() -> Self {
        Self {
            ampdu_frames: AtomicU32::new(0),
            not_ampdu_frames: AtomicU32::new(0),
            hardware_ampdu_frames: AtomicU32::new(0),
            hardware_not_ampdu_frames: AtomicU32::new(0),
            protocol_ampdu_frames: AtomicU32::new(0),
            protocol_not_ampdu_frames: AtomicU32::new(0),
            unavailable_frames: AtomicU32::new(0),
        }
    }

    pub fn observe(&self, evidence: MacRxEvidence<bool>) {
        let (total, provenance) = match evidence {
            MacRxEvidence::HardwareObserved(true) => {
                (&self.ampdu_frames, &self.hardware_ampdu_frames)
            }
            MacRxEvidence::HardwareObserved(false) => {
                (&self.not_ampdu_frames, &self.hardware_not_ampdu_frames)
            }
            MacRxEvidence::ProtocolValidated(true) => {
                (&self.ampdu_frames, &self.protocol_ampdu_frames)
            }
            MacRxEvidence::ProtocolValidated(false) => {
                (&self.not_ampdu_frames, &self.protocol_not_ampdu_frames)
            }
            MacRxEvidence::Unavailable => {
                self.unavailable_frames.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        total.fetch_add(1, Ordering::Relaxed);
        provenance.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RxAmpduSnapshot {
        RxAmpduSnapshot {
            ampdu_frames: self.ampdu_frames.load(Ordering::Relaxed),
            not_ampdu_frames: self.not_ampdu_frames.load(Ordering::Relaxed),
            hardware_ampdu_frames: self.hardware_ampdu_frames.load(Ordering::Relaxed),
            hardware_not_ampdu_frames: self.hardware_not_ampdu_frames.load(Ordering::Relaxed),
            protocol_ampdu_frames: self.protocol_ampdu_frames.load(Ordering::Relaxed),
            protocol_not_ampdu_frames: self.protocol_not_ampdu_frames.load(Ordering::Relaxed),
            unavailable_frames: self.unavailable_frames.load(Ordering::Relaxed),
        }
    }
}

impl Default for RxAmpduCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_keeps_hardware_protocol_and_unavailable_provenance_distinct() {
        let smpdu = RxSmpduCounters::new();
        smpdu.observe(MacRxEvidence::HardwareObserved(true));
        smpdu.observe(MacRxEvidence::ProtocolValidated(true));
        assert_eq!(smpdu.snapshot().s_mpdu_frames, 1);
        assert_eq!(smpdu.snapshot().unavailable_frames, 1);

        let ampdu = RxAmpduCounters::new();
        ampdu.observe(MacRxEvidence::HardwareObserved(true));
        ampdu.observe(MacRxEvidence::ProtocolValidated(true));
        ampdu.observe(MacRxEvidence::Unavailable);
        let snapshot = ampdu.snapshot();
        assert_eq!(snapshot.ampdu_frames, 2);
        assert_eq!(snapshot.hardware_ampdu_frames, 1);
        assert_eq!(snapshot.protocol_ampdu_frames, 1);
        assert_eq!(snapshot.unavailable_frames, 1);
    }
}
