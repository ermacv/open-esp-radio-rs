//! Independently requested PHY operations under exclusive maintenance access.
//!
//! Selection reuses the existing child transitions. It does not acknowledge a
//! complete periodic tracking evaluation or permit concurrent RF access.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Temperature,
    Rfpll,
    WifiPower,
    WifiI2c,
    CommonCalibration,
    WifiTxCalibration,
}

impl Operation {
    pub(crate) const fn calibration(self) -> super::calibration::Scope {
        match self {
            Self::CommonCalibration => super::calibration::Scope::Common,
            Self::WifiTxCalibration => super::calibration::Scope::Transmit,
            _ => super::calibration::Scope::Both,
        }
    }
}
