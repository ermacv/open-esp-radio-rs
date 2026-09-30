//! Coexistence priorities the ESP32-C5 radio clients publish.
//!
//! The table is the vendor cold table; the arbiter that changes it at run
//! time is not owned by this crate yet.

/// Number of coexistence events, event zero included.
pub const COEX_EVENT_COUNT: usize = 49;

/// Complete `coex_pti_tab` of esp-coex-lib
/// `c758e7b56e0fa22177a0539796e1df59978dc322` (`esp32c5/libcoexist.a`
/// sha256 `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`,
/// `coexist_core.o` section `.dram1.2`, 49 bytes), the pinned archive of
/// `verification/esp32c5/artifacts.toml`. The bytes equal the ESP32-S31
/// archive's table. Index is the event
/// number. Events 1, 3, 10 and 15 are the cold Wi-Fi MAC priorities (5, 7, 3
/// and 1); event 48 is the PHY grant-protect request (15).
const VENDOR_PTI_TABLE: [u8; COEX_EVENT_COUNT] = [
    0x0a, 0x05, 0x07, 0x07, 0x0a, 0x01, 0x01, 0x01, 0x01, 0x07, 0x03, 0x02, 0x01, 0x01, 0x01, 0x01,
    0x04, 0x09, 0x04, 0x04, 0x09, 0x04, 0x09, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x04, 0x04, 0x04,
    0x04, 0x02, 0x02, 0x02, 0x0f, 0x0a, 0x04, 0x0e, 0x00, 0x0c, 0x08, 0x03, 0x01, 0x0a, 0x0a, 0x0f,
    0x0f,
];

/// One coexistence event number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventId(u8);

impl CoexEventId {
    /// An event of the vendor table, or `None` outside it.
    pub const fn new(value: u8) -> Option<Self> {
        if (value as usize) < COEX_EVENT_COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// One four-bit coexistence priority, shared with the chip-neutral
/// IEEE 802.15.4 engine that publishes it to the MAC.
pub use oer_espressif_ieee802154_engine::coex::CoexPti;

/// Priority of every coexistence event.
///
/// The vendor coexistence scheduler changes entries at run time; the table
/// is therefore shared state of the arbiter, not a constant of any protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPtiTable([u8; COEX_EVENT_COUNT]);

impl CoexPtiTable {
    /// The vendor cold table.
    pub const VENDOR: Self = Self(VENDOR_PTI_TABLE);

    pub const fn pti(&self, event: CoexEventId) -> CoexPti {
        // Every byte is four-bit clean: the vendor table is, and `set` takes
        // only a checked priority.
        match CoexPti::new(self.0[event.0 as usize]) {
            Some(pti) => pti,
            None => panic!("the priority table holds only four-bit values"),
        }
    }

    pub fn set(&mut self, event: CoexEventId, pti: CoexPti) {
        self.0[event.0 as usize] = pti.value();
    }

    /// The priority of an IEEE 802.15.4 coexistence level.
    pub const fn ieee802154_pti(&self, level: Ieee802154CoexLevel) -> CoexPti {
        self.pti(level.event())
    }

    pub const fn as_bytes(&self) -> &[u8; COEX_EVENT_COUNT] {
        &self.0
    }
}

/// IEEE 802.15.4 coexistence priority level (`ieee802154_coex_event_t`).
///
/// The pinned archive's `coex_ieee802154_pti_get` reads the priority of
/// level `n` from the shared table at event `40 + n`
/// (esp-coex-lib `c758e7b56e0fa22177a0539796e1df59978dc322`,
/// `esp32c5/libcoexist.a` sha256
/// `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`,
/// `coexist_api.o`, whose body equals the ESP32-S31 one);
/// `esp_coex_ieee802154_txrx_pti_set` and `esp_coex_ieee802154_ack_pti_set`
/// publish that priority unchanged to the MAC's four-bit TX/RX and ACK PTI
/// fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Ieee802154CoexLevel {
    High = 1,
    Middle = 2,
    Low = 3,
    Idle = 4,
}

impl Ieee802154CoexLevel {
    /// The shared-table event that holds this level's priority.
    pub const fn event(self) -> CoexEventId {
        CoexEventId(40 + self as u8)
    }
}
