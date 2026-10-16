#![no_std]
#![forbid(unsafe_code)]

//! Radio APIs, portable protocols and explicitly selected hardware compositions.
//!
//! The default Wi-Fi API builds on the host without a chip or executor.
//! Firmware selects its chip and network composition through Cargo features.

pub use {memory, network};

#[cfg(feature = "wifi")]
pub mod ieee80211 {
    pub use {ap, datapath, mac, softmac, sta};

    pub mod security {
        pub use rsn;
    }

    /// Executor-independent drivers of the security state machines.
    pub mod services {
        pub use rsn_service as rsn;
    }
}

#[cfg(feature = "bluetooth")]
pub mod bluetooth {
    pub use hci;

    /// The bounded in-process HCI transport between Host and Controller.
    pub use hci_transport;

    pub mod le {
        pub use le_ll as ll;
    }
}

#[cfg(feature = "ieee802154")]
pub use ieee802154;

#[cfg(feature = "esp32s31")]
pub mod chips {
    pub mod esp32s31 {
        pub use chip_hal as hal;

        pub mod driver {
            /// Bluetooth hardware engine.
            #[cfg(feature = "esp32s31-bluetooth")]
            pub use chip_bluetooth as bluetooth;

            /// IEEE 802.15.4 MAC engine.
            #[cfg(feature = "esp32s31-ieee802154")]
            pub use chip_ieee802154 as ieee802154;
        }
    }
}

#[cfg(feature = "embassy-ieee802154")]
pub mod systems {
    pub mod esp32s31 {
        pub mod embassy {
            /// The shared radio every protocol composition joins, brought
            /// up with its periodic tasks by one `start` call.
            pub use radio_system as radio;

            /// The IEEE 802.15.4 client of the shared radio and, with
            /// `openthread`, its OpenThread radio.
            #[cfg(feature = "embassy-ieee802154")]
            pub mod ieee802154 {
                pub use ieee802154_composition::*;

                #[cfg(feature = "openthread")]
                pub use ieee802154_openthread as openthread;
            }
        }
    }
}
