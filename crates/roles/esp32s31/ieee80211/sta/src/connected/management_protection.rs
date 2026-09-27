//! Receive protection of one association's robust management frames.
//!
//! An association that negotiated management frame protection keeps the
//! pairwise temporal key in software to open its individually addressed
//! robust management frames, as the vendor does, and verifies group-addressed
//! ones under the IGTK with BIP.

use oer_ieee80211_rsn::{
    bip::{BipError, BipReceiver},
    frames::RsnIgtk,
    management_ccmp::{ManagementCcmpError, ManagementCcmpReceiver},
};

/// Management frame receive protection of one association.
pub struct StationManagementProtection {
    individual: ManagementCcmpReceiver,
    group: BipReceiver,
}

impl StationManagementProtection {
    /// Protection under the association's temporal key and its first IGTK.
    pub fn new(temporal_key: [u8; 16], igtk: &RsnIgtk) -> Self {
        Self {
            individual: ManagementCcmpReceiver::new(temporal_key),
            group: BipReceiver::new(igtk),
        }
    }

    /// Open one protected individually addressed robust management frame in
    /// place and return its body.
    pub fn open_individual<'a>(
        &mut self,
        frame: &'a mut [u8],
    ) -> Result<&'a [u8], ManagementCcmpError> {
        self.individual.open(frame)
    }

    /// Verify one group-addressed robust management frame.
    pub fn verify_group(&mut self, frame: &[u8]) -> Result<(), BipError> {
        self.group.verify(frame)
    }

    /// Follow a group rekey to its IGTK.
    pub fn install_igtk(&mut self, igtk: &RsnIgtk) {
        self.group.rekey(igtk);
    }
}
