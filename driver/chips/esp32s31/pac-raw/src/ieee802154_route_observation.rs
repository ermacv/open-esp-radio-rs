//! Read-only IEEE 802.15.4 interrupt-route observation.
//!
//! This module samples the two ESP32-S31 MODEM_ZB_MAC interrupt-map words
//! without exposing a pointer or any write operation. The read-only proof is
//! required by production polling: an ED/CCA transaction may temporarily
//! unmask MAC events only while both CPU routes remain exactly at reset. The
//! fixed addresses and field geometry are audited against ESP-IDF commit
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`:
//! `DR_REG_INTR0_BASE=0x20585000`, the core-one stride is `0x800`, and source
//! 132 has map-register offset `0x210` on both cores.

/// A fixed, aligned route-register address with no runtime constructor.
struct RouteRegister<const ADDRESS: usize>;

impl<const ADDRESS: usize> RouteRegister<ADDRESS> {
    const ADDRESS: usize = ADDRESS;

    #[inline]
    fn read(&self) -> u32 {
        // SAFETY: both const instantiations below are aligned, readable
        // ESP32-S31 interrupt-matrix register addresses proved by the pinned
        // public core0/core1 register headers. This read-only sidecar performs
        // a volatile read and exposes neither its pointer nor a write
        // operation.
        unsafe { core::ptr::read_volatile(Self::ADDRESS as *const u32) }
    }
}

const CORE0_MODEM_ZB_MAC_ROUTE: RouteRegister<0x2058_5210> = RouteRegister;
const CORE1_MODEM_ZB_MAC_ROUTE: RouteRegister<0x2058_5a10> = RouteRegister;

/// Complete ordered route-word observations for core zero and core one.
///
/// The two reads are ordered but not atomic. The raw words preserve reserved
/// bits for diagnostic evidence; semantic classification belongs to the pure
/// IEEE 802.15.4 IRQ crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RouteRawReadback {
    core0: u32,
    core1: u32,
}

impl Ieee802154RouteRawReadback {
    /// Return the complete core-zero register word.
    pub const fn core0_bits(self) -> u32 {
        self.core0
    }

    /// Return the complete core-one register word.
    pub const fn core1_bits(self) -> u32 {
        self.core1
    }
}

/// Sample the fixed source-132 route word on both CPU cores without writing.
///
/// The MAC peripheral reference anchors the call to the unique radio lease;
/// it is not used to derive either fixed route address.
#[inline]
pub fn read_route_words(_registers: &crate::Ieee802154Mac) -> Ieee802154RouteRawReadback {
    crate::device_access::fence();
    let core0 = CORE0_MODEM_ZB_MAC_ROUTE.read();
    let core1 = CORE1_MODEM_ZB_MAC_ROUTE.read();
    crate::device_access::fence();
    Ieee802154RouteRawReadback { core0, core1 }
}
