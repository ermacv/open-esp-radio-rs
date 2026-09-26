//! Channel-disjoint capabilities over the modem event-task matrix.
//!
//! IEEE 802.15.4 programs channels zero and one and the Bluetooth runtime
//! channels four through seven. The channel-enable set and clear words are
//! write-trigger — writing 1 acts on one channel and writing 0 has no effect,
//! as in the ESP32-S31 SoC ETM of identical layout; this is not yet verified
//! on the modem ETM hardware. Each capability therefore writes only its own
//! bits and never reads a trigger word, so the two protocols need no lock,
//! and IEEE 802.15.4 may use its channels from its interrupt handler.

use crate::svd;

/// ETM channels zero and one, owned by IEEE 802.15.4.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct Ieee802154EtmChannels {
    pub(crate) registers: svd::ModemEtm,
}

/// ETM channels four through seven, owned by the Bluetooth runtime.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct BluetoothEtmChannels {
    pub(crate) registers: svd::ModemEtm,
}

/// Split the unique modem ETM owner into the two channel capabilities.
///
/// # Safety invariant
///
/// The duplicated handle lives in [`BluetoothEtmChannels`], whose only
/// operation is the generated channel four-to-seven sequence;
/// [`Ieee802154EtmChannels`] writes only channel zero and one words and bits.
/// The capabilities are never reunited, so the complete block is never
/// recovered while both exist.
#[allow(
    unsafe_code,
    reason = "consuming the singleton creates the channel-disjoint capabilities"
)]
pub(crate) fn split(
    peripherals: svd::peripheral_ownership::ModemEtmPeripherals,
) -> (Ieee802154EtmChannels, BluetoothEtmChannels) {
    let svd::peripheral_ownership::ModemEtmPeripherals { modem_etm } = peripherals;
    // SAFETY: `modem_etm` was consumed above; see the invariant.
    let bluetooth = unsafe { svd::ModemEtm::steal() };
    (
        Ieee802154EtmChannels {
            registers: modem_etm,
        },
        BluetoothEtmChannels {
            registers: bluetooth,
        },
    )
}
