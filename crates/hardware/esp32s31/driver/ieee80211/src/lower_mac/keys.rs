//! Installed-key handles and the affine rights of admitted TX attempts.

use core::marker::PhantomData;
use oer_radio_port::{PoolPath, Root};

use super::{CheckedKeyScope, KeyToken, TxTicket};

type Installation<'core, Path> = PhantomData<fn(&'core (), Path) -> (&'core (), Path)>;

/// The sole handle of an installed key. Removing it consumes this value;
/// further admission then fails to compile. Already admitted attempts own
/// independent `KeyUse` values and may finish on different EDCA queues.
#[must_use = "remove the installed key through its core"]
pub struct KeyHandle<'core, Path: PoolPath = Root> {
    pub(super) index: usize,
    pub(super) token: KeyToken,
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

/// Permission to use the installed CCMP material for one admitted attempt.
///
/// Only admission through the installing core issues this right. It owns a
/// TX ticket, carries the installation brand and identifies both the logical
/// key slot and its programmed hardware selector. Publication reads that
/// selector and MIC size directly from the right; it never resolves a copied
/// key index. The secret material remains in that programmed hardware slot
/// until every right returns. No borrowed pointer into the core is retained.
///
/// The right travels with a held, published or quarantined attempt. A normal
/// terminal returns it exactly once. Poisoned keeps it in quarantine: neither
/// clearing the slot nor reporting `KeyRemoved` is permitted there.
#[must_use = "return the right only with its attempt's terminal or refusal"]
pub struct KeyUse<'core, Path: PoolPath = Root> {
    pub(super) ticket: TxTicket<'core, Path>,
    pub(super) index: usize,
    pub(super) selector: u8,
    pub(super) role: oer_ieee80211_lower_mac::VifRole,
}

/// The encryption authority of an admitted attempt. The protected variant
/// cannot be constructed from an index, a handle or a bare ticket.
#[must_use = "carry the admission to its terminal"]
pub enum TxAdmission<'core, Path: PoolPath = Root> {
    /// An unencrypted attempt still consumes one admission ticket.
    Open(TxTicket<'core, Path>),
    /// The right to use an installed key for this attempt alone.
    Protected(KeyUse<'core, Path>),
}

impl<Path: PoolPath> TxAdmission<'_, Path> {
    pub(super) fn matches_frame(
        &self,
        role: oer_ieee80211_lower_mac::VifRole,
        frame: Option<&[u8]>,
    ) -> bool {
        let Some(field) = frame.and_then(|frame| frame.first_chunk::<2>()) else {
            return false;
        };
        let control = oer_ieee80211_mac::frame::FrameControl::new(u16::from_le_bytes(*field));
        match self {
            Self::Open(_) => control.is_none_or(|control| !control.protected()),
            Self::Protected(key) => {
                key.role == role && control.is_some_and(|control| control.protected())
            }
        }
    }

    pub(super) const fn hardware(&self) -> (u8, usize) {
        match self {
            Self::Open(_) => (0, 0),
            Self::Protected(key) => (key.selector, crate::ordinary_tx::TX_CCMP_MIC_SIZE),
        }
    }
}

/// The key was cleared after its last admitted use returned. This receipt is
/// emitted at removal's FIFO position if no uses exist, otherwise immediately
/// after the last TX terminal. There is no polling step to discover completion.
#[must_use = "complete the corresponding portable key-removal request"]
pub struct KeyRemoved<'core, Path: PoolPath = Root> {
    pub(super) scope: CheckedKeyScope,
    pub(super) brand: Installation<'core, Path>,
}

impl<Path: PoolPath> KeyRemoved<'_, Path> {
    /// The installation's key which has now left the hardware.
    pub const fn scope(&self) -> CheckedKeyScope {
        self.scope
    }
}

/// Admission did not grant a key right; the caller retains its ticket.
#[must_use = "retain the returned ticket and handle the refusal"]
pub struct KeyAdmissionRefused<'core, Path: PoolPath = Root> {
    pub ticket: TxTicket<'core, Path>,
    pub reason: KeyAdmissionError,
}

/// Why an installed key cannot admit a TX attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAdmissionError {
    /// Station group keys only decrypt received group traffic.
    ReceiveOnly,
    /// The poisoned core retains its key material until reset.
    Poisoned,
}

/// An admission returned after the core was poisoned remains in custody.
/// It cannot be submitted again or turned into a reusable ticket.
#[must_use = "retain the admission in the poisoned backend's resource bank"]
pub struct QuarantinedAdmission<'core, Path: PoolPath = Root> {
    pub(super) _admission: TxAdmission<'core, Path>,
}
