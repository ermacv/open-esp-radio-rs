//! ESP32-S31 pre-admission ownership for legacy advertising.
//!
//! This boundary lowers one portable `ADV_NONCONN_IND` transmission into a
//! bounded PDU and an S31 primary-channel frequency choice. It deliberately
//! stops before scheduler admission: no reviewed advertising SRAM graph,
//! timeline policy, hardware-list role or completion contract exists yet.
//! Consequently this module cannot turn protocol work into `InFlight` or
//! publish scheduler `HEAD`/`RUN`.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::{
    advertiser::{
        LegacyAdvertiserEnabled, LegacyAdvertiserTxPrepared, LegacyAdvertisingTxIdentity,
    },
    advertising::{
        LEGACY_ADVERTISING_PDU_CAPACITY, LegacyAdvertisingEncodeError, PrimaryAdvertisingChannel,
    },
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxPacketPrepareError,
    BluetoothLeTxPacketPreparedLength, BluetoothLeTxPacketStorage,
};

const LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = LEGACY_ADVERTISING_PDU_CAPACITY - 2;
const LEGACY_ADVERTISING_TX_ALLOCATION_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

/// S31 packet-frequency image for one primary advertising channel.
///
/// The reviewed DTM frequency table uses the offset from 2402 MHz. Bluetooth
/// primary channels 37, 38 and 39 occupy 2402, 2426 and 2480 MHz respectively.
/// This is a future descriptor input, not an MMIO register image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingFrequency(u8);

impl BluetoothLegacyAdvertisingFrequency {
    const fn from_primary_channel(channel: PrimaryAdvertisingChannel) -> Self {
        match channel {
            PrimaryAdvertisingChannel::Channel37 => Self(0),
            PrimaryAdvertisingChannel::Channel38 => Self(24),
            PrimaryAdvertisingChannel::Channel39 => Self(78),
        }
    }

    /// Return the reviewed packet-frequency image for a future S31 descriptor owner.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// One fully encoded S31 legacy-advertising transmission before hardware admission.
///
/// The portable continuation remains private, so code cannot claim that the
/// transmission is in flight without first adding the missing sealed S31
/// hardware ticket at this boundary.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "admit through a reviewed hardware ticket, cancel, or retain the prepared owner"]
pub struct BluetoothLegacyAdvertisingPrepared<'a> {
    prepared: LegacyAdvertiserTxPrepared<'a>,
    packet: BluetoothLeTxPacketStorage<LEGACY_ADVERTISING_TX_ALLOCATION_BYTES>,
    packet_length: BluetoothLeTxPacketPreparedLength<LEGACY_ADVERTISING_TX_ALLOCATION_BYTES>,
    frequency: BluetoothLegacyAdvertisingFrequency,
}

impl<'a> BluetoothLegacyAdvertisingPrepared<'a> {
    /// Encode the next portable channel transmission into bounded chip-owned storage.
    pub fn prepare(
        enabled: LegacyAdvertiserEnabled<'a>,
    ) -> Result<Self, BluetoothLegacyAdvertisingPreparationError<'a>> {
        let prepared = enabled.prepare_next();
        let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let pdu_len = match prepared.encode(&mut encoded) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::PduEncoding(error),
                });
            }
        };
        let mut packet = BluetoothLeTxPacketStorage::new();
        let packet_length = match packet.prepare_encoded_pdu(&encoded[..pdu_len]) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::ControllerPacket(error),
                });
            }
        };
        let frequency = BluetoothLegacyAdvertisingFrequency::from_primary_channel(
            prepared.identity().channel(),
        );
        Ok(Self {
            prepared,
            packet,
            packet_length,
            frequency,
        })
    }

    /// Exact portable generation/event/channel identity retained by this owner.
    pub const fn identity(&self) -> LegacyAdvertisingTxIdentity {
        self.prepared.identity()
    }

    /// Selected primary advertising channel.
    pub const fn channel(&self) -> PrimaryAdvertisingChannel {
        self.identity().channel()
    }

    /// Complete encoded Link Layer PDU, excluding preamble, Access Address, CRC and whitening.
    pub fn pdu(&self) -> &[u8] {
        self.packet.prepared_pdu(self.packet_length)
    }

    /// Typed future descriptor input for the selected primary channel.
    pub const fn frequency(&self) -> BluetoothLegacyAdvertisingFrequency {
        self.frequency
    }

    /// Cancel before hardware admission and recover the exact enabled event.
    pub fn cancel(self) -> LegacyAdvertiserEnabled<'a> {
        self.prepared.cancel()
    }
}

/// Bounded PDU encoding failed before any S31 hardware ownership changed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the unchanged enabled advertiser remains recoverable"]
pub struct BluetoothLegacyAdvertisingPreparationError<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    error: BluetoothLegacyAdvertisingPreparationErrorKind,
}

impl<'a> BluetoothLegacyAdvertisingPreparationError<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingPreparationErrorKind {
        self.error
    }

    pub fn into_enabled(self) -> LegacyAdvertiserEnabled<'a> {
        self.enabled
    }
}

/// Finite CPU-side reason why one advertising transmission was not prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingPreparationErrorKind {
    /// The portable Link Layer PDU producer rejected its bounded destination.
    PduEncoding(LegacyAdvertisingEncodeError),
    /// The complete PDU did not satisfy the common controller TX allocation.
    ControllerPacket(BluetoothLeTxPacketPrepareError),
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertiser::LegacyAdvertiserStandby,
        advertising::{
            AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
            LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannel,
            PrimaryAdvertisingChannelMap,
        },
    };

    use super::BluetoothLegacyAdvertisingPrepared;

    fn enabled(
        channels: PrimaryAdvertisingChannelMap,
    ) -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
        let advertisement = LegacyNonconnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
            LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits legacy advertising"),
        );
        LegacyAdvertiserStandby::new()
            .configure(LegacyNonconnectableAdvertisingSet::new(
                advertisement,
                channels,
                AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
                    .expect("the minimum interval is valid"),
            ))
            .enable()
            .expect("the first generation is available")
    }

    #[test]
    fn preparation_retains_identity_and_cancel_restores_the_same_channel() {
        let prepared = BluetoothLegacyAdvertisingPrepared::prepare(enabled(
            PrimaryAdvertisingChannelMap::all(),
        ))
        .expect("bounded validated advertising data always fits the chip PDU");
        let identity = prepared.identity();

        assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        assert_eq!(prepared.channel(), PrimaryAdvertisingChannel::Channel37);
        assert_eq!(prepared.cancel().prepare_next().identity(), identity);
    }

    #[test]
    fn primary_channels_lower_to_the_reviewed_s31_frequency_domain() {
        for (channels, channel, frequency) in [
            (
                PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
                PrimaryAdvertisingChannel::Channel37,
                0,
            ),
            (
                PrimaryAdvertisingChannelMap::new(false, true, false).unwrap(),
                PrimaryAdvertisingChannel::Channel38,
                24,
            ),
            (
                PrimaryAdvertisingChannelMap::new(false, false, true).unwrap(),
                PrimaryAdvertisingChannel::Channel39,
                78,
            ),
        ] {
            let prepared = BluetoothLegacyAdvertisingPrepared::prepare(enabled(channels))
                .expect("bounded validated advertising data always fits the chip PDU");
            assert_eq!(prepared.channel(), channel);
            assert_eq!(prepared.frequency().get(), frequency);
        }
    }
}
