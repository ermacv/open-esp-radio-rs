//! Portable IEEE 802.11 PHY rates.
//!
//! [`PhyRate`] names the PPDU format and modulation of one transmission or
//! reception: a non-HT rate, an HT MCS or an HE SU MCS with the parameters
//! that select it. It is what a radio port submits and reports; each backend
//! lowers it into its own rate codes and refuses what its PHY cannot send.
//! Every constructor validates the combination, so a held value is a rate
//! IEEE 802.11-2020/802.11ax-2021 defines.

/// Preamble of a DSSS or CCK PPDU (IEEE 802.11-2020 16.2.2).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DsssPreamble {
    Long,
    Short,
}

/// Non-HT rate: DSSS, CCK (clause 16) or OFDM/ERP-OFDM (clauses 17, 18).
///
/// 1 Mb/s has only the long preamble; the short preamble's header is sent at
/// 2 Mb/s.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LegacyRate {
    Dsss1M,
    Dsss2M(DsssPreamble),
    Cck5M5(DsssPreamble),
    Cck11M(DsssPreamble),
    Ofdm6M,
    Ofdm9M,
    Ofdm12M,
    Ofdm18M,
    Ofdm24M,
    Ofdm36M,
    Ofdm48M,
    Ofdm54M,
}

impl LegacyRate {
    /// The data rate in kb/s.
    pub const fn kbps(self) -> u32 {
        match self {
            Self::Dsss1M => 1_000,
            Self::Dsss2M(_) => 2_000,
            Self::Cck5M5(_) => 5_500,
            Self::Cck11M(_) => 11_000,
            Self::Ofdm6M => 6_000,
            Self::Ofdm9M => 9_000,
            Self::Ofdm12M => 12_000,
            Self::Ofdm18M => 18_000,
            Self::Ofdm24M => 24_000,
            Self::Ofdm36M => 36_000,
            Self::Ofdm48M => 48_000,
            Self::Ofdm54M => 54_000,
        }
    }

    /// Whether the rate is OFDM rather than DSSS or CCK.
    pub const fn is_ofdm(self) -> bool {
        !matches!(
            self,
            Self::Dsss1M | Self::Dsss2M(_) | Self::Cck5M5(_) | Self::Cck11M(_)
        )
    }
}

/// Bandwidth of one PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PpduBandwidth {
    Mhz20,
    Mhz40,
    Mhz80,
    /// 160 MHz or 80+80 MHz.
    Mhz160,
}

impl PpduBandwidth {
    pub const fn mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40 => 40,
            Self::Mhz80 => 80,
            Self::Mhz160 => 160,
        }
    }
}

/// HT MCS index 0 through 31 (IEEE 802.11-2020 19.5): the equal-modulation
/// MCSs of one to four spatial streams. MCS 32 (duplicate) and the
/// unequal-modulation MCSs 33-76 are not ordinary rates of this type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HtMcs(u8);

impl HtMcs {
    pub const fn new(index: u8) -> Option<Self> {
        if index <= 31 { Some(Self(index)) } else { None }
    }

    pub const fn index(self) -> u8 {
        self.0
    }

    /// The spatial streams the MCS uses.
    pub const fn spatial_streams(self) -> u8 {
        self.0 / 8 + 1
    }
}

/// One HT rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HtRate {
    mcs: HtMcs,
    bandwidth: PpduBandwidth,
    short_gi: bool,
}

impl HtRate {
    /// An HT rate; `None` for a bandwidth HT does not have (80 or 160 MHz).
    pub const fn new(mcs: HtMcs, bandwidth: PpduBandwidth, short_gi: bool) -> Option<Self> {
        match bandwidth {
            PpduBandwidth::Mhz20 | PpduBandwidth::Mhz40 => Some(Self {
                mcs,
                bandwidth,
                short_gi,
            }),
            PpduBandwidth::Mhz80 | PpduBandwidth::Mhz160 => None,
        }
    }

    pub const fn mcs(self) -> HtMcs {
        self.mcs
    }

    pub const fn bandwidth(self) -> PpduBandwidth {
        self.bandwidth
    }

    /// 400 ns rather than 800 ns guard interval.
    pub const fn short_gi(self) -> bool {
        self.short_gi
    }
}

/// HE MCS index 0 through 11 (IEEE 802.11ax-2021 27.5).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeMcs(u8);

impl HeMcs {
    pub const fn new(index: u8) -> Option<Self> {
        if index <= 11 { Some(Self(index)) } else { None }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Number of spatial streams, one through eight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpatialStreams(u8);

impl SpatialStreams {
    pub const ONE: Self = Self(1);

    pub const fn new(count: u8) -> Option<Self> {
        if count >= 1 && count <= 8 {
            Some(Self(count))
        } else {
            None
        }
    }

    pub const fn count(self) -> u8 {
        self.0
    }
}

/// HE-LTF size and guard interval of an HE SU PPDU: the four combinations
/// the GI+LTF Size field of HE-SIG-A selects (IEEE 802.11ax-2021 Table 27-18).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HeGiLtf {
    /// 1x HE-LTF with a 0.8 µs guard interval.
    Ltf1xGi800Ns,
    /// 2x HE-LTF with a 0.8 µs guard interval.
    Ltf2xGi800Ns,
    /// 2x HE-LTF with a 1.6 µs guard interval.
    Ltf2xGi1600Ns,
    /// 4x HE-LTF with a 3.2 µs guard interval.
    Ltf4xGi3200Ns,
}

impl HeGiLtf {
    pub const fn guard_interval_ns(self) -> u16 {
        match self {
            Self::Ltf1xGi800Ns | Self::Ltf2xGi800Ns => 800,
            Self::Ltf2xGi1600Ns => 1_600,
            Self::Ltf4xGi3200Ns => 3_200,
        }
    }
}

/// Forward error correction of an HT or HE PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FecCoding {
    Bcc,
    Ldpc,
}

/// One HE SU rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeRate {
    mcs: HeMcs,
    spatial_streams: SpatialStreams,
    bandwidth: PpduBandwidth,
    gi_ltf: HeGiLtf,
    fec: FecCoding,
    dcm: bool,
}

impl HeRate {
    /// An HE SU rate; `None` for dual carrier modulation outside MCS 0, 1, 3
    /// and 4 with one or two spatial streams (IEEE 802.11ax-2021 27.3.12.10).
    pub const fn new(
        mcs: HeMcs,
        spatial_streams: SpatialStreams,
        bandwidth: PpduBandwidth,
        gi_ltf: HeGiLtf,
        fec: FecCoding,
        dcm: bool,
    ) -> Option<Self> {
        if dcm && (!matches!(mcs.0, 0 | 1 | 3 | 4) || spatial_streams.0 > 2) {
            return None;
        }
        Some(Self {
            mcs,
            spatial_streams,
            bandwidth,
            gi_ltf,
            fec,
            dcm,
        })
    }

    pub const fn mcs(self) -> HeMcs {
        self.mcs
    }

    pub const fn spatial_streams(self) -> SpatialStreams {
        self.spatial_streams
    }

    pub const fn bandwidth(self) -> PpduBandwidth {
        self.bandwidth
    }

    pub const fn gi_ltf(self) -> HeGiLtf {
        self.gi_ltf
    }

    pub const fn fec(self) -> FecCoding {
        self.fec
    }

    /// Dual carrier modulation.
    pub const fn dcm(self) -> bool {
        self.dcm
    }
}

/// The PHY rate of one PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PhyRate {
    Legacy(LegacyRate),
    Ht(HtRate),
    He(HeRate),
}

impl PhyRate {
    /// The PPDU bandwidth; a non-HT PPDU occupies 20 MHz.
    pub const fn bandwidth(self) -> PpduBandwidth {
        match self {
            Self::Legacy(_) => PpduBandwidth::Mhz20,
            Self::Ht(rate) => rate.bandwidth,
            Self::He(rate) => rate.bandwidth,
        }
    }
}

#[cfg(test)]
mod tests;
