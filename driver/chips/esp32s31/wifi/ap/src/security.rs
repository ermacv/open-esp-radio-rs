//! Typed ownership of one AP GTK and the bounded pairwise CCMP slot table.

use open_esp_radio_esp32s31_wifi_mac::crypto::{
    AP_PAIRWISE_SLOT_COUNT, ApGroupCcmpSlot, ApPairwiseCcmpSlot, CcmpTxPacketNumberError,
    CryptoKeyError, install_ap_group_ccmp, install_ap_pairwise_ccmp,
};
use open_esp_radio_ieee80211::ccmp::{
    CcmpPacketNumber, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate, CcmpRxReplayState,
};
use open_esp_radio_wpa2::{Ptk, frames::Wpa2Gtk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApSecurityError {
    Crypto(CryptoKeyError),
    PacketNumber(CcmpTxPacketNumberError),
    SecurityModeMismatch,
    PairwiseStorageNotEmpty,
    PairwiseAlreadyInstalled,
    AssociationIdAlreadyInstalled,
    WrongPeer,
    Replay(CcmpReplayError),
}

impl From<CryptoKeyError> for Esp32s31ApSecurityError {
    fn from(error: CryptoKeyError) -> Self {
        Self::Crypto(error)
    }
}

impl From<CcmpTxPacketNumberError> for Esp32s31ApSecurityError {
    fn from(error: CcmpTxPacketNumberError) -> Self {
        Self::PacketNumber(error)
    }
}

impl From<CcmpReplayError> for Esp32s31ApSecurityError {
    fn from(error: CcmpReplayError) -> Self {
        Self::Replay(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApSecurityStopReport {
    OpenNoKeys,
    Wpa2Personal {
        pairwise_slots_cleared: u8,
        group_hardware_index: u8,
    },
}

/// O(1) identity of one installed pairwise hardware-key slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApPairwiseBinding {
    index: u8,
    peer: [u8; 6],
    hardware_index: u8,
    generation: u32,
}

impl Esp32s31ApPairwiseBinding {
    pub const fn hardware_index(self) -> u8 {
        self.hardware_index
    }

    pub(crate) const fn generation(self) -> u32 {
        self.generation
    }
}

/// Two-phase replay admission tied to one exact pairwise-key generation.
///
/// The binding is revalidated at commit, so a candidate prepared before a
/// peer clear or PTK replacement cannot mutate the replacement key's replay
/// frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApPairwiseRxCandidate {
    binding: Esp32s31ApPairwiseBinding,
    replay: CcmpRxReplayCandidate,
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
    generations: [u32; AP_PAIRWISE_SLOT_COUNT as usize],
    rx_replay: [CcmpRxReplayState; AP_PAIRWISE_SLOT_COUNT as usize],
}

impl Esp32s31ApPairwiseKeyStorage {
    pub const fn new() -> Self {
        Self {
            pairwise: [const { None }; AP_PAIRWISE_SLOT_COUNT as usize],
            generations: [0; AP_PAIRWISE_SLOT_COUNT as usize],
            rx_replay: [const { CcmpRxReplayState::new(CcmpPacketNumber::ZERO) };
                AP_PAIRWISE_SLOT_COUNT as usize],
        }
    }
}

impl Default for Esp32s31ApPairwiseKeyStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub enum Esp32s31ApSecurity<'storage> {
    Open {
        storage: Option<&'storage mut Esp32s31ApPairwiseKeyStorage>,
    },
    Wpa2Personal {
        group: ApGroupCcmpSlot,
        storage: Option<&'storage mut Esp32s31ApPairwiseKeyStorage>,
    },
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
        Ok(Self::Wpa2Personal {
            group,
            storage: Some(storage),
        })
    }

    /// Bind an Open AP epoch without touching any hardware key entry.
    pub fn open(
        storage: &'storage mut Esp32s31ApPairwiseKeyStorage,
    ) -> Result<Self, Esp32s31ApSecurityStartFailure<'storage>> {
        if storage.pairwise.iter().any(Option::is_some) {
            return Err(Esp32s31ApSecurityStartFailure {
                error: Esp32s31ApSecurityError::PairwiseStorageNotEmpty,
                storage,
            });
        }
        Ok(Self::Open {
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
        if matches!(self, Self::Open { .. }) {
            return Err(Esp32s31ApSecurityError::SecurityModeMismatch);
        }
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
            .slots()
            .get(index)
            .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?;
        if destination.is_some() {
            return Err(Esp32s31ApSecurityError::AssociationIdAlreadyInstalled);
        }
        let next_generation = self.storage().generations[index]
            .checked_add(1)
            .expect("AP pairwise-key generation space is not reusable");
        let slot = install_ap_pairwise_ccmp(hardware, peer, association_id, ptk.temporal_key())?;
        let storage = self.storage_mut();
        storage.pairwise[index] = Some(slot);
        storage.generations[index] = next_generation;
        storage.rx_replay[index] = CcmpRxReplayState::default();
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
        let storage = self.storage_mut();
        storage.generations[index] = storage.generations[index]
            .checked_add(1)
            .expect("AP pairwise-key generation space is not reusable");
        storage.rx_replay[index] = CcmpRxReplayState::default();
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
        if matches!(self, Self::Open { .. }) {
            return Err(Esp32s31ApSecurityError::SecurityModeMismatch);
        }
        let slot = self
            .slots_mut()
            .iter_mut()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        Ok(slot.next_tx_ccmp_header()?)
    }

    pub fn bind_pairwise(
        &self,
        peer: [u8; 6],
        association_id: u16,
    ) -> Result<Esp32s31ApPairwiseBinding, Esp32s31ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(Esp32s31ApSecurityError::SecurityModeMismatch);
        }
        let index = usize::from(
            u8::try_from(
                association_id
                    .checked_sub(1)
                    .ok_or(CryptoKeyError::InvalidAccessPointAssociationId)?,
            )
            .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
        );
        let slot = self
            .slots()
            .get(index)
            .and_then(Option::as_ref)
            .filter(|slot| slot.peer() == &peer)
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        Ok(Esp32s31ApPairwiseBinding {
            index: u8::try_from(index)
                .map_err(|_| CryptoKeyError::InvalidAccessPointAssociationId)?,
            peer,
            hardware_index: slot.hardware_index(),
            generation: self.storage().generations[index],
        })
    }

    /// Prepare one post-reorder, hardware-authenticated pairwise CCMP MPDU.
    ///
    /// Callers must commit the returned token before publishing any Ethernet
    /// view. The token remains fenced to this exact installed PTK generation.
    pub fn prepare_bound_pairwise_rx(
        &self,
        binding: Esp32s31ApPairwiseBinding,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<Esp32s31ApPairwiseRxCandidate, Esp32s31ApSecurityError> {
        let index = self.validate_pairwise_binding(binding)?;
        let replay = self.storage().rx_replay[index].prepare(lane, packet_number)?;
        Ok(Esp32s31ApPairwiseRxCandidate { binding, replay })
    }

    /// Commit a prepared PN only while its pairwise key generation is still
    /// current. A clear/reinstall edge invalidates the candidate first.
    pub fn commit_bound_pairwise_rx(
        &mut self,
        candidate: Esp32s31ApPairwiseRxCandidate,
    ) -> Result<(), Esp32s31ApSecurityError> {
        let index = self.validate_pairwise_binding(candidate.binding)?;
        self.storage_mut().rx_replay[index].commit(candidate.replay)?;
        Ok(())
    }

    pub fn next_bound_pairwise_tx_ccmp_header(
        &mut self,
        binding: Esp32s31ApPairwiseBinding,
    ) -> Result<[u8; 8], Esp32s31ApSecurityError> {
        let index = self.validate_pairwise_binding(binding)?;
        let slot = self
            .slots_mut()
            .get_mut(index)
            .and_then(Option::as_mut)
            .expect("validated pairwise binding owns an occupied slot");
        Ok(slot.next_tx_ccmp_header()?)
    }

    pub fn pairwise_hardware_index(&self, peer: [u8; 6]) -> Result<u8, Esp32s31ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(Esp32s31ApSecurityError::SecurityModeMismatch);
        }
        let slot = self
            .slots()
            .iter()
            .flatten()
            .find(|slot| slot.peer() == &peer)
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        Ok(slot.hardware_index())
    }

    pub fn next_group_tx_ccmp_header(&mut self) -> Result<[u8; 8], Esp32s31ApSecurityError> {
        match self {
            Self::Open { .. } => Err(Esp32s31ApSecurityError::SecurityModeMismatch),
            Self::Wpa2Personal { group, .. } => Ok(group.next_tx_ccmp_header()?),
        }
    }

    pub const fn group_hardware_index(&self) -> Result<u8, Esp32s31ApSecurityError> {
        match self {
            Self::Open { .. } => Err(Esp32s31ApSecurityError::SecurityModeMismatch),
            Self::Wpa2Personal { group, .. } => Ok(group.hardware_index()),
        }
    }

    /// Clear every installed AP key before the radio owner may become stopped.
    pub fn stop<H>(
        self,
        hardware: &mut H,
    ) -> (
        Esp32s31ApSecurityStopReport,
        &'storage mut Esp32s31ApPairwiseKeyStorage,
    )
    where
        H: open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware,
    {
        let (storage, group) = match self {
            Self::Open { mut storage } => (
                storage
                    .take()
                    .expect("active Open AP security owns pairwise-key storage"),
                None,
            ),
            Self::Wpa2Personal { group, mut storage } => (
                storage
                    .take()
                    .expect("active WPA2 AP security owns pairwise-key storage"),
                Some(group),
            ),
        };
        let mut pairwise_slots_cleared = 0_u8;
        for index in 0..storage.pairwise.len() {
            if let Some(pairwise) = storage.pairwise[index].take() {
                pairwise.clear(hardware);
                storage.generations[index] = storage.generations[index]
                    .checked_add(1)
                    .expect("AP pairwise-key generation space is not reusable");
                storage.rx_replay[index] = CcmpRxReplayState::default();
                pairwise_slots_cleared = pairwise_slots_cleared.saturating_add(1);
            }
        }
        let report = if let Some(group) = group {
            let group_hardware_index = group.hardware_index();
            group.clear(hardware);
            Esp32s31ApSecurityStopReport::Wpa2Personal {
                pairwise_slots_cleared,
                group_hardware_index,
            }
        } else {
            debug_assert_eq!(pairwise_slots_cleared, 0);
            Esp32s31ApSecurityStopReport::OpenNoKeys
        };
        (report, storage)
    }

    fn slots(&self) -> &[Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref()
        .expect("active AP security owns pairwise-key storage")
        .pairwise
    }

    fn slots_mut(&mut self) -> &mut [Option<ApPairwiseCcmpSlot>; AP_PAIRWISE_SLOT_COUNT as usize] {
        &mut self.storage_mut().pairwise
    }

    fn storage(&self) -> &Esp32s31ApPairwiseKeyStorage {
        match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref()
        .expect("active AP security owns pairwise-key storage")
    }

    fn storage_mut(&mut self) -> &mut Esp32s31ApPairwiseKeyStorage {
        match self {
            Self::Open { storage } | Self::Wpa2Personal { storage, .. } => storage,
        }
        .as_deref_mut()
        .expect("active AP security owns pairwise-key storage")
    }

    fn validate_pairwise_binding(
        &self,
        binding: Esp32s31ApPairwiseBinding,
    ) -> Result<usize, Esp32s31ApSecurityError> {
        if matches!(self, Self::Open { .. }) {
            return Err(Esp32s31ApSecurityError::SecurityModeMismatch);
        }
        let index = usize::from(binding.index);
        let storage = self.storage();
        let slot = storage
            .pairwise
            .get(index)
            .and_then(Option::as_ref)
            .filter(|slot| {
                slot.peer() == &binding.peer
                    && slot.hardware_index() == binding.hardware_index
                    && storage.generations[index] == binding.generation
            })
            .ok_or(Esp32s31ApSecurityError::WrongPeer)?;
        debug_assert_eq!(slot.hardware_index(), binding.hardware_index);
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
    use open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware;
    use open_esp_radio_wpa2::{Pmk, PtkContext};

    #[derive(Default)]
    struct Hardware {
        installs: std::vec::Vec<u8>,
        clears: std::vec::Vec<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(
            &mut self,
            index: u8,
            _identity: open_esp_radio_esp32s31_hal::types::MacCcmpKeyIdentity,
            _temporal_key: &[u8; 16],
        ) -> MacKeyInstallOutcome {
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
        let binding = security.bind_pairwise([3; 6], 1).unwrap();
        assert_eq!(binding.hardware_index(), 8);
        assert_eq!(
            security
                .next_bound_pairwise_tx_ccmp_header(binding)
                .unwrap(),
            [3, 0, 0, 0x20, 0, 0, 0, 0]
        );
        assert_eq!(
            security.bind_pairwise([3; 6], 2),
            Err(Esp32s31ApSecurityError::WrongPeer)
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
        assert_eq!(
            report,
            Esp32s31ApSecurityStopReport::Wpa2Personal {
                pairwise_slots_cleared: 2,
                group_hardware_index: 2,
            }
        );
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
        assert_eq!(
            report,
            Esp32s31ApSecurityStopReport::Wpa2Personal {
                pairwise_slots_cleared: 15,
                group_hardware_index: 2,
            }
        );
        assert_eq!(
            hardware.clears,
            (8..=22)
                .chain(core::iter::once(2))
                .collect::<std::vec::Vec<_>>()
        );
    }

    #[test]
    fn rx_replay_is_per_tid_and_fenced_across_pairwise_key_reinstall() {
        let peer = [3; 6];
        let mut hardware = Hardware::default();
        let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
        let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
        let mut security =
            Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
        let ptk = Pmk::derive(b"password", b"ssid")
            .unwrap()
            .derive_ptk(PtkContext {
                authenticator_address: [2; 6],
                supplicant_address: peer,
                authenticator_nonce: [4; 32],
                supplicant_nonce: [5; 32],
            });
        security
            .install_pairwise(&mut hardware, peer, 1, &ptk)
            .unwrap();
        let first_generation = security.bind_pairwise(peer, 1).unwrap();
        let pn1 = CcmpPacketNumber::new(1).unwrap();
        let pn4 = CcmpPacketNumber::new(4).unwrap();
        let pn5 = CcmpPacketNumber::new(5).unwrap();

        let first = security
            .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4)
            .unwrap();
        security.commit_bound_pairwise_rx(first).unwrap();
        assert_eq!(
            security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4,),
            Err(Esp32s31ApSecurityError::Replay(CcmpReplayError::Replayed {
                packet_number: pn4,
                highest: pn4,
            })),
            "a different 802.11 sequence cannot make a reused PN admissible"
        );

        for lane in [CcmpReplayLane::Tid(1), CcmpReplayLane::Tid(7)] {
            let candidate = security
                .prepare_bound_pairwise_rx(first_generation, lane, pn1)
                .unwrap();
            security.commit_bound_pairwise_rx(candidate).unwrap();
        }

        let prepared_before_clear = security
            .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5)
            .unwrap();
        security.clear_peer(&mut hardware, peer).unwrap();
        security
            .install_pairwise(&mut hardware, peer, 1, &ptk)
            .unwrap();
        let replacement_generation = security.bind_pairwise(peer, 1).unwrap();
        assert_ne!(replacement_generation, first_generation);
        assert_eq!(
            security.commit_bound_pairwise_rx(prepared_before_clear),
            Err(Esp32s31ApSecurityError::WrongPeer),
            "a candidate from the cleared key cannot advance its replacement"
        );
        assert_eq!(
            security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5,),
            Err(Esp32s31ApSecurityError::WrongPeer),
        );

        let replacement = security
            .prepare_bound_pairwise_rx(replacement_generation, CcmpReplayLane::NonQos, pn1)
            .unwrap();
        security.commit_bound_pairwise_rx(replacement).unwrap();
    }
}
