//! ESP32-S31 pre-admission ownership for legacy advertising.
//!
//! This boundary lowers one portable `ADV_NONCONN_IND` transmission into a
//! bounded PDU and an S31 primary-channel frequency choice. It deliberately
//! stops before scheduler admission: the SRAM allocation is now bound, but no
//! reviewed advertising link-state image, timeline policy, hardware-list role
//! or completion contract exists yet.
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
    BluetoothLeTxPacketPrepareError, BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
};

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
#[must_use = "admit through a reviewed hardware ticket, cancel, or retain the prepared owner"]
pub struct BluetoothLegacyAdvertisingPrepared<'a> {
    prepared: LegacyAdvertiserTxPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
    frequency: BluetoothLegacyAdvertisingFrequency,
}

impl<'a> BluetoothLegacyAdvertisingPrepared<'a> {
    /// Encode the next portable channel transmission into bounded chip-owned storage.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return both exact affine owners"
    )]
    pub fn prepare(
        enabled: LegacyAdvertiserEnabled<'a>,
        memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    ) -> Result<Self, BluetoothLegacyAdvertisingPreparationError<'a>> {
        let prepared = enabled.prepare_next();
        let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let pdu_len = match prepared.encode(&mut encoded) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    memory,
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::PduEncoding(error),
                });
            }
        };
        let memory = match memory.prepare_packet(&encoded[..pdu_len]) {
            Ok(memory) => memory,
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    memory,
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::ControllerPacket(error),
                });
            }
        };
        let frequency = BluetoothLegacyAdvertisingFrequency::from_primary_channel(
            prepared.identity().channel(),
        );
        Ok(Self {
            prepared,
            memory,
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
        self.memory.pdu()
    }

    /// Typed future descriptor input for the selected primary channel.
    pub const fn frequency(&self) -> BluetoothLegacyAdvertisingFrequency {
        self.frequency
    }

    /// Cancel before hardware admission and recover both affine owners.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        BluetoothLegacyAdvertisingCancelled {
            enabled: self.prepared.cancel(),
            memory: self.memory.cancel(),
        }
    }
}

/// Lossless cancellation before any advertising descriptor is publishable.
#[must_use = "both the portable advertiser and bound SRAM graph must be retained"]
pub struct BluetoothLegacyAdvertisingCancelled<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
}

impl<'a> BluetoothLegacyAdvertisingCancelled<'a> {
    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserEnabled<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    ) {
        (self.enabled, self.memory)
    }
}

/// Bounded PDU encoding failed before any S31 hardware ownership changed.
#[must_use = "the unchanged advertiser and SRAM graph remain recoverable"]
pub struct BluetoothLegacyAdvertisingPreparationError<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    error: BluetoothLegacyAdvertisingPreparationErrorKind,
}

impl<'a> BluetoothLegacyAdvertisingPreparationError<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingPreparationErrorKind {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserEnabled<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingPreparationErrorKind,
    ) {
        (self.enabled, self.memory, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingPreparationError<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingPreparationError")
            .field("error", &self.error)
            .finish_non_exhaustive()
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
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyAdvertisingMemoryGraphStorage,
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

    fn memory() -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the advertising graph fits physical controller SRAM")
    }

    #[test]
    fn preparation_retains_identity_and_cancel_restores_the_same_channel() {
        let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
            enabled(PrimaryAdvertisingChannelMap::all()),
            memory(),
        )
        .expect("bounded validated advertising data always fits the chip PDU");
        let identity = prepared.identity();

        assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        assert_eq!(prepared.channel(), PrimaryAdvertisingChannel::Channel37);
        let (enabled, _memory) = prepared.cancel().into_parts();
        assert_eq!(enabled.prepare_next().identity(), identity);
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
            let prepared = BluetoothLegacyAdvertisingPrepared::prepare(enabled(channels), memory())
                .expect("bounded validated advertising data always fits the chip PDU");
            assert_eq!(prepared.channel(), channel);
            assert_eq!(prepared.frequency().get(), frequency);
        }
    }
}
