//! The ESP32-S31 IEEE 802.15.4 capabilities markers: what the port's
//! validated values are checked against ([`Ieee802154Caps`]), for a
//! single-interface engine and for a multi-PAN one.

use oer_espressif_ieee802154_engine::{engine::PENDING_TABLE_SIZE, pib::CCA_THRESHOLD_RANGE_DBM};
use oer_espressif_ieee802154_runtime::{
    IEEE802154_ENHANCED_ACK_IE_CAPACITY, IEEE802154_RADIO_CAPABILITIES, Ieee802154RuntimeCaps,
};
use oer_ieee802154::{
    CcaThresholdCapability, Ieee802154Capabilities, RadioCapabilities, TransmitPowerCapability,
    owner::{Ieee802154Caps, InterfaceResources},
};
use oer_radio_port::{Child, PoolPath, Root};

/// The CCA threshold the radio starts with: the PIB default.
const CCA_RESET_DBM: i8 = -75;

const fn capabilities(operations: RadioCapabilities, interfaces: u8) -> Ieee802154Capabilities {
    let levels = oer_esp32s31_hal::ieee802154::ESP32S31_TX_POWER_LEVELS.portable();
    Ieee802154Capabilities {
        operations,
        interfaces,
        transmit_power: Some(TransmitPowerCapability {
            levels,
            reset_dbm: levels.highest_dbm(),
        }),
        cca_threshold: Some(CcaThresholdCapability {
            lowest_dbm: *CCA_THRESHOLD_RANGE_DBM.start(),
            highest_dbm: *CCA_THRESHOLD_RANGE_DBM.end(),
            reset_dbm: CCA_RESET_DBM,
        }),
        receive_sensitivity_dbm: oer_espressif_ieee802154_engine::engine::RECEIVE_SENSITIVITY_DBM,
        csl: Some(oer_espressif_ieee802154_runtime::IEEE802154_CSL_CAPABILITY),
        enhanced_ack_header_ies: IEEE802154_ENHANCED_ACK_IE_CAPACITY as u8,
    }
}

/// A single-interface ESP32-S31 radio ([`Ieee802154Parked::new`](crate::Ieee802154Parked::new)).
pub enum Esp32s31Ieee802154 {}

impl Ieee802154Caps for Esp32s31Ieee802154 {
    const CAPS: Ieee802154Capabilities = capabilities(IEEE802154_RADIO_CAPABILITIES, 1);
}

impl Ieee802154RuntimeCaps for Esp32s31Ieee802154 {
    type Interfaces = oer_ieee802154::owner::InterfacePool<Self, 0, PENDING_TABLE_SIZE>;
    type InterfacePath<const I: u8> = Child<Root, 0>;
}

/// A multi-PAN ESP32-S31 radio of four interfaces
/// ([`Ieee802154Parked::multipan`](crate::Ieee802154Parked::multipan)).
pub enum Esp32s31Ieee802154Multipan {}

impl Ieee802154Caps for Esp32s31Ieee802154Multipan {
    const CAPS: Ieee802154Capabilities = capabilities(
        IEEE802154_RADIO_CAPABILITIES.union(RadioCapabilities::MULTI_PAN),
        4,
    );
}

/// The four interfaces' resources of a multi-PAN radio.
pub struct MultipanInterfaces<'id, Path: PoolPath> {
    /// Interface 0.
    pub interface0: InterfaceResources<'id, 0, PENDING_TABLE_SIZE, Child<Path, 0>>,
    /// Interface 1.
    pub interface1: InterfaceResources<'id, 1, PENDING_TABLE_SIZE, Child<Path, 1>>,
    /// Interface 2.
    pub interface2: InterfaceResources<'id, 2, PENDING_TABLE_SIZE, Child<Path, 2>>,
    /// Interface 3.
    pub interface3: InterfaceResources<'id, 3, PENDING_TABLE_SIZE, Child<Path, 3>>,
}

impl Ieee802154RuntimeCaps for Esp32s31Ieee802154Multipan {
    type Interfaces = Self;
    type InterfacePath<const I: u8> = Child<Child<Root, 0>, I>;
}

impl oer_radio_port::PoolSet for Esp32s31Ieee802154Multipan {
    type Pools<'id, Path: PoolPath> = MultipanInterfaces<'id, Path>;

    fn build<'id, Path: PoolPath>(
        builder: oer_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let (interface0, interface1, interface2, interface3) = builder.into_pools::<(
            oer_ieee802154::owner::InterfacePool<Self, 0, PENDING_TABLE_SIZE>,
            oer_ieee802154::owner::InterfacePool<Self, 1, PENDING_TABLE_SIZE>,
            oer_ieee802154::owner::InterfacePool<Self, 2, PENDING_TABLE_SIZE>,
            oer_ieee802154::owner::InterfacePool<Self, 3, PENDING_TABLE_SIZE>,
        )>();
        MultipanInterfaces {
            interface0,
            interface1,
            interface2,
            interface3,
        }
    }
}
