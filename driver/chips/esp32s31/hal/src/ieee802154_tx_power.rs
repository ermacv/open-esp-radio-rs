//! Source-only ESP32-S31 IEEE 802.15.4 transmit-power resolution.
//!
//! The public vendor PIB supplies the conversion control flow but obtains the
//! actual ordered dBm levels from an external BTBB provider. This module ports
//! only that public control flow. It validates caller-provided levels and
//! produces an opaque field code without embedding a chip table, touching
//! MMIO, or claiming that an arbitrary register image is calibrated.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::marker::PhantomData;

use crate::ieee802154_lifecycle::Ieee802154Channel;

const MAX_PROVIDER_LEVEL_COUNT: usize = u8::MAX as usize;

/// Why an externally supplied transmit-power level set cannot be resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154TxPowerLevelsError {
    /// The provider supplied no levels.
    Empty,
    /// The provider supplied more levels than its public eight-bit length can
    /// represent.
    TooLong {
        /// Complete rejected level count.
        length: usize,
        /// Largest accepted level count.
        maximum: usize,
    },
    /// A later level was lower than its predecessor.
    Descending {
        /// Index of the lower, rejected level.
        index: usize,
        /// Level immediately before the rejected value.
        previous_dbm: i8,
        /// Rejected lower level.
        current_dbm: i8,
    },
}

/// One validated external sequence of supported transmit-power levels.
///
/// The sequence must contain between one and 255 entries and be
/// non-decreasing. Equal adjacent values are retained because the public PIB
/// branch order makes their selection deterministic: a request at or below
/// the first level selects index zero, a request at or above the final level
/// selects the final index, and the interior floor scan selects the greatest
/// matching index. Validation proves only that the open conversion can execute
/// without truncation; it does not prove where the values came from or that RF
/// calibration is ready.
/// In particular, this borrow tracks storage lifetime, not a provider or
/// calibration epoch; arbitrary external levels grant no hardware authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxPowerLevels<'levels> {
    levels_dbm: &'levels [i8],
}

impl<'levels> Ieee802154TxPowerLevels<'levels> {
    /// Validate one externally supplied level sequence without copying it.
    pub const fn new(levels_dbm: &'levels [i8]) -> Result<Self, Ieee802154TxPowerLevelsError> {
        if levels_dbm.is_empty() {
            return Err(Ieee802154TxPowerLevelsError::Empty);
        }
        if levels_dbm.len() > MAX_PROVIDER_LEVEL_COUNT {
            return Err(Ieee802154TxPowerLevelsError::TooLong {
                length: levels_dbm.len(),
                maximum: MAX_PROVIDER_LEVEL_COUNT,
            });
        }

        let mut index = 1;
        while index < levels_dbm.len() {
            if levels_dbm[index] < levels_dbm[index - 1] {
                return Err(Ieee802154TxPowerLevelsError::Descending {
                    index,
                    previous_dbm: levels_dbm[index - 1],
                    current_dbm: levels_dbm[index],
                });
            }
            index += 1;
        }

        Ok(Self { levels_dbm })
    }

    /// Return the validated external level count.
    pub const fn len(self) -> usize {
        self.levels_dbm.len()
    }

    /// A validated provider level set is never empty.
    pub const fn is_empty(self) -> bool {
        false
    }

    /// Resolve one request with the public vendor clamp-and-floor scan.
    ///
    /// A request at or below the first provider value selects index zero, and
    /// one at or above the final value selects the final index. Otherwise the
    /// selected field code is the greatest provider index whose dBm value does
    /// not exceed `requested_dbm`. This branch precedence is observable when
    /// boundary values are duplicated. The result retains the channel so
    /// later integration cannot silently separate channel and power selection.
    pub const fn resolve(
        self,
        channel: Ieee802154Channel,
        requested_dbm: i8,
    ) -> Ieee802154ResolvedTxPower<'levels> {
        let last_index = self.levels_dbm.len() - 1;
        let selected_index = if requested_dbm <= self.levels_dbm[0] {
            0
        } else if requested_dbm >= self.levels_dbm[last_index] {
            last_index
        } else {
            let mut index = last_index;
            while index != 0 && self.levels_dbm[index] > requested_dbm {
                index -= 1;
            }
            index
        };

        Ieee802154ResolvedTxPower {
            channel,
            requested_dbm,
            effective_dbm: self.levels_dbm[selected_index],
            field_code: Ieee802154TxPowerCode(selected_index as u8),
            _levels: PhantomData,
        }
    }
}

/// Internal field code selected from one validated provider level set.
///
/// Neither this type nor its constructor is public. Keeping it non-`Copy`
/// prevents the provider-relative code from being detached from the
/// channel-bound resolution that owns it; it does not turn arbitrary levels
/// into a provider, calibration epoch, or MMIO capability.
#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Ieee802154TxPowerCode(u8);

impl Ieee802154TxPowerCode {
    const fn value(&self) -> u8 {
        self.0
    }
}

/// One channel-bound result of the source-only transmit-power scan.
///
/// The lifetime prevents the result from outliving the external level storage
/// from which it was derived. It is not a fabricated provider or calibration
/// epoch; operational code must add that ownership boundary before writing
/// the retained field code.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::{
///     Ieee802154Channel, Ieee802154ResolvedTxPower,
/// };
///
/// let channel = Ieee802154Channel::new(20).unwrap();
/// let forged = Ieee802154ResolvedTxPower::from_raw_parts(channel, 0, 0, 7);
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::{
///     Ieee802154Channel, Ieee802154TxPowerLevels,
/// };
///
/// let escaped = {
///     let provider_levels = [-9, 4];
///     let levels = Ieee802154TxPowerLevels::new(&provider_levels).unwrap();
///     let channel = Ieee802154Channel::new(20).unwrap();
///     levels.resolve(channel, 0)
/// };
/// let _ = escaped.selected_provider_index();
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct Ieee802154ResolvedTxPower<'levels> {
    channel: Ieee802154Channel,
    requested_dbm: i8,
    effective_dbm: i8,
    field_code: Ieee802154TxPowerCode,
    _levels: PhantomData<&'levels [i8]>,
}

impl Ieee802154ResolvedTxPower<'_> {
    /// Return the channel retained by this resolution.
    pub const fn channel(&self) -> Ieee802154Channel {
        self.channel
    }

    /// Return the caller's request before provider-relative clamping.
    pub const fn requested_dbm(&self) -> i8 {
        self.requested_dbm
    }

    /// Return the provider level selected by the public scan.
    pub const fn effective_dbm(&self) -> i8 {
        self.effective_dbm
    }

    /// Return the selected external-provider index for diagnostics.
    ///
    /// This numeric observation grants no register-write authority. A future
    /// HAL writer must consume the complete channel-bound resolution rather
    /// than accept this value or the internal field code separately.
    pub const fn selected_provider_index(&self) -> u8 {
        self.field_code().value()
    }

    pub(crate) const fn field_code(&self) -> &Ieee802154TxPowerCode {
        &self.field_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> Ieee802154Channel {
        Ieee802154Channel::new(20).expect("standard channel")
    }

    fn reference_index(levels: &[i8], requested_dbm: i8) -> usize {
        if requested_dbm <= levels[0] {
            return 0;
        }
        if requested_dbm >= levels[levels.len() - 1] {
            return levels.len() - 1;
        }

        let mut index = levels.len() - 1;
        while index != 0 {
            if levels[index] <= requested_dbm {
                break;
            }
            index -= 1;
        }
        index
    }

    #[test]
    fn level_validation_fails_closed_before_resolution() {
        assert_eq!(
            Ieee802154TxPowerLevels::new(&[]),
            Err(Ieee802154TxPowerLevelsError::Empty)
        );

        let too_long = [0; MAX_PROVIDER_LEVEL_COUNT + 1];
        assert_eq!(
            Ieee802154TxPowerLevels::new(&too_long),
            Err(Ieee802154TxPowerLevelsError::TooLong {
                length: MAX_PROVIDER_LEVEL_COUNT + 1,
                maximum: MAX_PROVIDER_LEVEL_COUNT,
            })
        );

        let descending = [-91, -12, 6, -4, 77];
        assert_eq!(
            Ieee802154TxPowerLevels::new(&descending),
            Err(Ieee802154TxPowerLevelsError::Descending {
                index: 3,
                previous_dbm: 6,
                current_dbm: -4,
            })
        );
    }

    #[test]
    fn complete_source_length_domain_and_duplicate_levels_are_accepted() {
        let single = [9];
        let duplicates = [-83, -7, -7, 42];
        let maximum = [5; MAX_PROVIDER_LEVEL_COUNT];

        assert_eq!(
            Ieee802154TxPowerLevels::new(&single)
                .expect("single provider level")
                .len(),
            1
        );
        assert_eq!(
            Ieee802154TxPowerLevels::new(&duplicates)
                .expect("non-decreasing provider levels")
                .len(),
            duplicates.len()
        );
        let maximum =
            Ieee802154TxPowerLevels::new(&maximum).expect("largest public provider length");
        assert_eq!(maximum.len(), MAX_PROVIDER_LEVEL_COUNT);
        assert!(!maximum.is_empty());
        assert_eq!(
            maximum
                .resolve(channel(), i8::MAX)
                .selected_provider_index(),
            u8::MAX - 1
        );
    }

    #[test]
    fn resolution_matches_the_public_scan_for_every_i8_request() {
        let synthetic_sets: [&[i8]; 4] = [
            &[11],
            &[i8::MIN, -95, -13, 2, 37, i8::MAX],
            &[-103, -8, -8, 19, 71],
            &[-64, -31, 0, 1, 56, 92],
        ];
        let channel = channel();

        for raw_levels in synthetic_sets {
            let levels = Ieee802154TxPowerLevels::new(raw_levels)
                .expect("synthetic levels are non-decreasing");
            for requested_dbm in i8::MIN..=i8::MAX {
                let expected_index = reference_index(raw_levels, requested_dbm);
                let resolved = levels.resolve(channel, requested_dbm);

                assert_eq!(resolved.channel(), channel);
                assert_eq!(resolved.requested_dbm(), requested_dbm);
                assert_eq!(resolved.selected_provider_index(), expected_index as u8);
                assert_eq!(resolved.effective_dbm(), raw_levels[expected_index]);
            }
        }
    }

    #[test]
    fn equal_levels_select_the_greatest_matching_index() {
        let raw_levels = [-83, -7, -7, -7, 42];
        let levels = Ieee802154TxPowerLevels::new(&raw_levels)
            .expect("duplicates preserve non-decreasing order");
        let resolved = levels.resolve(channel(), -7);

        assert_eq!(resolved.selected_provider_index(), 3);
        assert_eq!(resolved.effective_dbm(), -7);
    }

    #[test]
    fn duplicated_boundary_levels_follow_public_branch_precedence() {
        let raw_levels = [-7, -7, 0, 42, 42];
        let levels = Ieee802154TxPowerLevels::new(&raw_levels)
            .expect("duplicates preserve non-decreasing order");

        let minimum = levels.resolve(channel(), -7);
        assert_eq!(minimum.selected_provider_index(), 0);
        assert_eq!(minimum.effective_dbm(), -7);

        let above_minimum = levels.resolve(channel(), -6);
        assert_eq!(above_minimum.selected_provider_index(), 1);
        assert_eq!(above_minimum.effective_dbm(), -7);

        let maximum = levels.resolve(channel(), 42);
        assert_eq!(maximum.selected_provider_index(), 4);
        assert_eq!(maximum.effective_dbm(), 42);
    }
}
