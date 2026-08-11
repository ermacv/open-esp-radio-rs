//! Typed ownership of the first AP GTK and one client pairwise CCMP slot.

use open_esp_radio_esp32s31_wifi_mac::crypto::{
    ApGroupCcmpSlot, ApPairwiseCcmpSlot, CryptoKeyError, install_ap_group_ccmp,
    install_ap_pairwise_ccmp,
};
use open_esp_radio_wpa2::{Ptk, frames::Wpa2Gtk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApSecurityError {
    Crypto(CryptoKeyError),
    PairwiseAlreadyInstalled,
    WrongPeer,
}

impl From<CryptoKeyError> for Esp32s31ApSecurityError {
    fn from(error: CryptoKeyError) -> Self {
        Self::Crypto(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApSecurityStopReport {
    pub pairwise_slot_cleared: bool,
    pub group_hardware_index: u8,
}

pub struct Esp32s31ApSecurity {
    group: ApGroupCcmpSlot,
    pairwise: Option<ApPairwiseCcmpSlot>,
}

impl Esp32s31ApSecurity {
    pub fn install_group<H>(hardware: &mut H, gtk: &Wpa2Gtk) -> Result<Self, CryptoKeyError>
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        let group = install_ap_group_ccmp(hardware, gtk.key_id(), gtk.key())?;
        Ok(Self {
            group,
            pairwise: None,
        })
    }

    pub fn install_pairwise<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ptk: &Ptk,
    ) -> Result<(), Esp32s31ApSecurityError>
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        if self.pairwise.is_some() {
            return Err(Esp32s31ApSecurityError::PairwiseAlreadyInstalled);
        }
        self.pairwise = Some(install_ap_pairwise_ccmp(
            hardware,
            peer,
            ptk.temporal_key(),
        )?);
        Ok(())
    }

    pub fn clear_peer<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), Esp32s31ApSecurityError>
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        let Some(slot) = self.pairwise.take() else {
            return Ok(());
        };
        if slot.peer() != &peer {
            self.pairwise = Some(slot);
            return Err(Esp32s31ApSecurityError::WrongPeer);
        }
        slot.clear(hardware);
        Ok(())
    }

    /// Allocate the next pairwise TX packet number for the owned peer.
    ///
    /// A failed later frame build may leave a gap, which is valid for CCMP;
    /// the packet number is never rolled back or reused.
    pub fn next_pairwise_tx_ccmp_header(
        &mut self,
        peer: [u8; 6],
    ) -> Result<[u8; 8], Esp32s31ApSecurityError> {
        let slot = self
            .pairwise
            .as_mut()
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        if slot.peer() != &peer {
            return Err(Esp32s31ApSecurityError::WrongPeer);
        }
        Ok(slot.next_tx_ccmp_header())
    }

    pub fn pairwise_hardware_index(&self, peer: [u8; 6]) -> Result<u8, Esp32s31ApSecurityError> {
        let slot = self
            .pairwise
            .as_ref()
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        if slot.peer() != &peer {
            return Err(Esp32s31ApSecurityError::WrongPeer);
        }
        Ok(slot.hardware_index())
    }

    /// Clear every installed AP key before the radio owner may become stopped.
    pub fn stop<H>(self, hardware: &mut H) -> Esp32s31ApSecurityStopReport
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        let pairwise_slot_cleared = if let Some(pairwise) = self.pairwise {
            pairwise.clear(hardware);
            true
        } else {
            false
        };
        let group_hardware_index = self.group.hardware_index();
        self.group.clear(hardware);
        Esp32s31ApSecurityStopReport {
            pairwise_slot_cleared,
            group_hardware_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_registers::MacKeyInstallOutcome;
    use open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware;
    use open_esp_radio_wpa2::{Pmk, PtkContext};

    #[derive(Default)]
    struct Hardware {
        installs: std::vec::Vec<u8>,
        clears: std::vec::Vec<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            self.installs.push(index);
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.clears.push(index);
        }
    }

    #[test]
    fn teardown_clears_pairwise_before_group_and_cannot_alias_a_second_peer() {
        let mut hardware = Hardware::default();
        let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
        let mut security = Esp32s31ApSecurity::install_group(&mut hardware, &gtk).unwrap();
        let ptk = Pmk::derive(b"password", b"ssid")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: [2; 6],
                supplicant_address: [3; 6],
                authenticator_nonce: [4; 32],
                supplicant_nonce: [5; 32],
            });
        security
            .install_pairwise(&mut hardware, [3; 6], &ptk)
            .unwrap();
        assert_eq!(
            security.next_pairwise_tx_ccmp_header([3; 6]).unwrap(),
            [3, 0, 0, 0x20, 0, 0, 0, 0]
        );
        assert_eq!(security.pairwise_hardware_index([3; 6]), Ok(8));
        assert_eq!(
            security.install_pairwise(&mut hardware, [6; 6], &ptk),
            Err(Esp32s31ApSecurityError::PairwiseAlreadyInstalled)
        );
        let report = security.stop(&mut hardware);
        assert_eq!(hardware.installs, [2, 8]);
        assert_eq!(hardware.clears, [8, 2]);
        assert!(report.pairwise_slot_cleared);
        assert_eq!(report.group_hardware_index, 2);
    }
}
