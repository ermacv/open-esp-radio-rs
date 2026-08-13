//! Typed ownership of one AP GTK and the bounded pairwise CCMP slot table.

use open_esp_radio_esp32s31_wifi_mac::crypto::{
    AP_PAIRWISE_SLOT_COUNT, ApGroupCcmpSlot, ApPairwiseCcmpSlot, CryptoKeyError,
    install_ap_group_ccmp, install_ap_pairwise_ccmp,
};
use open_esp_radio_wpa2::{Ptk, frames::Wpa2Gtk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApSecurityError {
    Crypto(CryptoKeyError),
    PairwiseStorageNotEmpty,
    PairwiseAlreadyInstalled,
    AssociationIdAlreadyInstalled,
    WrongPeer,
}

impl From<CryptoKeyError> for Esp32s31ApSecurityError {
    fn from(error: CryptoKeyError) -> Self {
        Self::Crypto(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApSecurityStopReport {
    pub pairwise_slots_cleared: u8,
    pub group_hardware_index: u8,
}

pub struct Esp32s31ApSecurityStartFailure<'storage> {
    pub error: Esp32s31ApSecurityError,
    pub storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
}

impl core::fmt::Debug for Esp32s31ApSecurityStartFailure<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Esp32s31ApSecurityStartFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Stable caller-owned table of installed AP pairwise-key capabilities.
pub struct Esp32s31ApPairwiseKeyStorage {
    pairwise: [Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize],
}

impl Esp32s31ApPairwiseKeyStorage {
    pub const fn new() -> Self {
        Self {
            pairwise: [const { None }; AP_PAIRWISE_SLOT_COUNT as usize],
        }
    }
}

impl Default for Esp32s31ApPairwiseKeyStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Esp32s31ApSecurity<'storage> {
    group: ApGroupCcmpSlot,
    storage: Option<&'storage mut Esp32s31ApPairwiseKeyStorage>,
}

impl<'storage> Esp32s31ApSecurity<'storage> {
    pub fn install_group<H>(
        hardware: &mut H,
        gtk: &Wpa2Gtk,
        storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    ) -> Result<Self, Esp32s31ApSecurityStartFailure<'storage>>
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        if storage.pairwise.iter().any(Option::is_some) {
            return Err(Esp32s31ApSecurityStartFailure {
                error: Esp32s31ApSecurityError::PairwiseStorageNotEmpty,
                storage,
            });
        }
        let group = match install_ap_group_ccmp(hardware, gtk.key_id(), gtk.key()) {
            Ok(group) => group,
            Err(error) => {
                return Err(Esp32s31ApSecurityStartFailure {
                    error: Esp32s31ApSecurityError::Crypto(error),
                    storage,
                });
            }
        };
        Ok(Self {
            group,
            storage: Some(storage),
        })
    }

    pub fn install_pairwise<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        association_id: u16,
        ptk: &Ptk,
    ) -> Result<(), Esp32s31ApSecurityError>
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        if self
            .slots()
            .iter()
            .flatten()
            .any(|slot| slot.peer() == &peer)
        {
            return Err(Esp32s31ApSecurityError::PairwiseAlreadyInstalled);
        }
        let index = usize::from(
            u8::try_from(
                association_id
                    .checked_sub(1)
                    .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?,
            )
            .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
        );
        let destination = self
            .slots_mut()
            .get_mut(index)
            .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?;
        if destination.is_some() {
            return Err(Esp32s31ApSecurityError::AssociationIdAlreadyInstalled);
        }
        *destination = Some(install_ap_pairwise_ccmp(
            hardware,
            peer,
            association_id,
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
        let Some(index) = self
            .slots()
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|slot| slot.peer() == &peer))
        else {
            return Ok(());
        };
        let slot = self.slots_mut()[index]
            .take()
            .expect("matching pairwise slot is occupied");
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
            .slots_mut()
            .iter_mut()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        Ok(slot.next_tx_ccmp_header())
    }

    pub fn pairwise_hardware_index(&self, peer: [u8; 6]) -> Result<u8, Esp32s31ApSecurityError> {
        let slot = self
            .slots()
            .iter()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        Ok(slot.hardware_index())
    }

    pub fn next_group_tx_ccmp_header(&mut self) -> [u8; 8] {
        self.group.next_tx_ccmp_header()
    }

    pub const fn group_hardware_index(&self) -> u8 {
        self.group.hardware_index()
    }

    /// Clear every installed AP key before the radio owner may become stopped.
    pub fn stop<H>(
        mut self,
        hardware: &mut H,
    ) -> (
        Esp32s31ApSecurityStopReport,
        &'storage mut Esp32s31ApPairwiseKeyStorage,
    )
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        let storage = self
            .storage
            .take()
            .expect("active AP security owns pairwise-key storage");
        let mut pairwise_slots_cleared = 0_u8;
        for pairwise in storage.pairwise.iter_mut().filter_map(Option::take) {
            pairwise.clear(hardware);
            pairwise_slots_cleared = pairwise_slots_cleared.saturating_add(1);
        }
        let group_hardware_index = self.group.hardware_index();
        self.group.clear(hardware);
        (
            Esp32s31ApSecurityStopReport {
                pairwise_slots_cleared,
                group_hardware_index,
            },
            storage,
        )
    }

    fn slots(&self) -> &[Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &self
            .storage
            .as_deref()
            .expect("active AP security owns pairwise-key storage")
            .pairwise
    }

    fn slots_mut(&mut self) -> &mut [Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &mut self
            .storage
            .as_deref_mut()
            .expect("active AP security owns pairwise-key storage")
            .pairwise
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_pac::MacKeyInstallOutcome;
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
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let mut security =
            Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
        let ptk = Pmk::derive(b"password", b"ssid")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: [2; 6],
                supplicant_address: [3; 6],
                authenticator_nonce: [4; 32],
                supplicant_nonce: [5; 32],
            });
        security
            .install_pairwise(&mut hardware, [3; 6], 1, &ptk)
            .unwrap();
        assert_eq!(
            security.next_pairwise_tx_ccmp_header([3; 6]).unwrap(),
            [3, 0, 0, 0x20, 0, 0, 0, 0]
        );
        assert_eq!(security.pairwise_hardware_index([3; 6]), Ok(8));
        assert_eq!(
            security.install_pairwise(&mut hardware, [3; 6], 2, &ptk),
            Err(Esp32s31ApSecurityError::PairwiseAlreadyInstalled),
        );
        security
            .install_pairwise(&mut hardware, [6; 6], 2, &ptk)
            .unwrap();
        let (report, _pairwise) = security.stop(&mut hardware);
        assert_eq!(hardware.installs, [2, 8, 9]);
        assert_eq!(hardware.clears, [8, 9, 2]);
        assert_eq!(report.pairwise_slots_cleared, 2);
        assert_eq!(report.group_hardware_index, 2);
    }

    #[test]
    fn all_public_aids_own_disjoint_pairwise_slots() {
        let mut hardware = Hardware::default();
        let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let mut security =
            Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
        let ptk = Pmk::derive(b"password", b"ssid")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: [2; 6],
                supplicant_address: [3; 6],
                authenticator_nonce: [4; 32],
                supplicant_nonce: [5; 32],
            });

        for association_id in 1..=15_u16 {
            let peer = [2, 0, 0, 0, 1, association_id as u8];
            security
                .install_pairwise(&mut hardware, peer, association_id, &ptk)
                .unwrap();
            assert_eq!(
                security.pairwise_hardware_index(peer),
                Ok(7 + association_id as u8),
            );
        }
        assert_eq!(
            hardware.installs,
            (2..=22)
                .filter(|index| *index == 2 || *index >= 8)
                .collect::<std::vec::Vec<_>>()
        );

        let overflow = security.install_pairwise(&mut hardware, [2, 0, 0, 0, 2, 1], 16, &ptk);
        assert_eq!(
            overflow,
            Err(Esp32s31ApSecurityError::Crypto(
                CryptoKeyError::InvalidAccessPointAssociationId,
            )),
        );
        let (report, _pairwise) = security.stop(&mut hardware);
        assert_eq!(report.pairwise_slots_cleared, 15);
        assert_eq!(
            hardware.clears,
            (8..=22)
                .chain(core::iter::once(2))
                .collect::<std::vec::Vec<_>>()
        );
    }
}
