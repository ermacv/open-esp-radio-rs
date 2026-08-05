//! Validated A-MPDU frame geometry and rate-policy requests.

use crate::tx::{HeEdcaTxopLimit, HeRate, HtAmpduDensity, HtRate};

/// Geometry of one encoded MPDU inside a retained DMA backing.
///
/// The offset points at the private A-MPDU metadata prefix immediately before
/// the encoded 802.11 frame. Construction proves the word alignment required
/// by the S31 TX descriptor path. This value describes a region; it does not
/// grant access to it. [`super::RetainedDmaAmpduTx`] remains the sole owner
/// allowed to resolve the offset inside a retained
/// [`open_esp_radio_dma::StableDmaBacking`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduFrameLayout {
    dma_offset: usize,
    mpdu_length: usize,
    hardware_mic_length: u8,
}

impl AmpduFrameLayout {
    pub const fn new(
        dma_offset: usize,
        mpdu_length: usize,
        hardware_mic_length: u8,
    ) -> Option<Self> {
        if dma_offset & 3 != 0 {
            return None;
        }
        Some(Self {
            dma_offset,
            mpdu_length,
            hardware_mic_length,
        })
    }

    pub const fn dma_offset(self) -> usize {
        self.dma_offset
    }

    pub const fn mpdu_length(self) -> usize {
        self.mpdu_length
    }

    pub const fn hardware_mic_length(self) -> u8 {
        self.hardware_mic_length
    }
}

/// Complete policy required to append one retained HT MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduFrameRequest {
    layout: AmpduFrameLayout,
    empty_delimiters: u8,
    rate: HtRate,
}

impl HtAmpduFrameRequest {
    pub const fn new(layout: AmpduFrameLayout, empty_delimiters: u8, rate: HtRate) -> Self {
        Self {
            layout,
            empty_delimiters,
            rate,
        }
    }

    pub const fn layout(self) -> AmpduFrameLayout {
        self.layout
    }

    pub const fn empty_delimiters(self) -> u8 {
        self.empty_delimiters
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }
}

/// Complete policy required to append one retained HE MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeAmpduFrameRequest {
    layout: AmpduFrameLayout,
    rate: HeRate,
    density: HtAmpduDensity,
    txop_limit: HeEdcaTxopLimit,
}

impl HeAmpduFrameRequest {
    pub const fn new(
        layout: AmpduFrameLayout,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Self {
        Self {
            layout,
            rate,
            density,
            txop_limit,
        }
    }

    pub const fn layout(self) -> AmpduFrameLayout {
        self.layout
    }

    pub const fn rate(self) -> HeRate {
        self.rate
    }

    pub const fn density(self) -> HtAmpduDensity {
        self.density
    }

    pub const fn txop_limit(self) -> HeEdcaTxopLimit {
        self.txop_limit
    }
}
