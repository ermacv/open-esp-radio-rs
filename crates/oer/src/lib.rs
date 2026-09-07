#![no_std]
#![forbid(unsafe_code)]

//! Radio APIs, portable protocols and explicitly selected hardware compositions.
//!
//! The default Wi-Fi API builds on the host without a chip or executor.
//! Firmware selects its chip and network composition through Cargo features.

pub use {memory, network, radio};

#[cfg(feature = "wifi")]
pub use radio::wifi;

#[cfg(feature = "wifi")]
pub mod ieee80211 {
    pub use {ap, datapath, mac, softmac, sta};

    pub mod security {
        pub use wpa2;
    }
}

#[cfg(feature = "bluetooth")]
pub mod bluetooth {
    pub use hci;

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
            #[cfg(feature = "esp32s31-bluetooth")]
            pub use chip_bluetooth as bluetooth;

            #[cfg(feature = "esp32s31-wifi")]
            pub mod ieee80211 {
                pub use {chip_ap as ap, chip_mac as mac, chip_sta as sta};
            }
        }
    }
}

#[cfg(any(
    feature = "upstream-xarxa",
    feature = "owned-xarxa",
    feature = "embassy-smoltcp",
    feature = "embassy-esp32s31-bluetooth"
))]
pub mod systems {
    pub mod esp32s31 {
        pub mod embassy {
            #[cfg(any(
                feature = "upstream-xarxa",
                feature = "owned-xarxa",
                feature = "embassy-smoltcp"
            ))]
            pub use wifi_composition as wifi;

            /// Controller composition; API availability does not establish RF readiness.
            #[cfg(feature = "embassy-esp32s31-bluetooth")]
            pub use bluetooth_composition as bluetooth;
        }
    }
}
