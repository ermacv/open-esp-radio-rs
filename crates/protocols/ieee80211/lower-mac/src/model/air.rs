//! The air between host models.
//!
//! [`ModelAir`] joins several [`LowerMacModel`]s as radios sharing a medium,
//! so a test runs real drivers against each other instead of scripting one
//! side: an access point on one model, its stations on others. Each
//! [`ModelAir::step`] carries every attempt the radios have published: each
//! other enabled radio on the sender's primary channel receives its MPDUs
//! as its interfaces' filters admit them, and the attempt ends at once,
//! [`TxStatus::Success`] when it solicits no response or a radio on the
//! channel has an interface of its first address,
//! [`TxStatus::AckTimeout`] otherwise. A control frame (a CTS-to-self)
//! solicits no response. The model ciphers nothing: a protected frame
//! arrives decrypted and verified when the receiver holds a pairwise key of
//! its transmitter, or for a group frame a group key, and is not received
//! otherwise. An overheard CTS-to-self holds other radios' published
//! attempts until its Duration expires; the harness advances to
//! [`LowerMacModel::nav_until`] as well as its services' deadlines.
//! Air time, contention, loss and reception levels are not
//! modelled: every frame on the channel arrives, at -40 dBm.

use alloc::vec::Vec;

use super::LowerMacModel;
use crate::{
    Channel, KeyScope, RxCryptoStatus, RxEvidence, RxMeta, TxBody, TxStatus, VifId,
    model::ModelOutcome,
};

/// Radios sharing one medium; see the module documentation.
pub struct ModelAir<'m, O: TxBody> {
    radios: Vec<&'m LowerMacModel<O>>,
}

impl<'m, O: TxBody> ModelAir<'m, O> {
    pub fn new(radios: impl IntoIterator<Item = &'m LowerMacModel<O>>) -> Self {
        Self {
            radios: radios.into_iter().collect(),
        }
    }

    /// Carry every published attempt once; whether any was.
    pub fn step(&self) -> bool {
        let mut carried = false;
        for (index, sender) in self.radios.iter().enumerate() {
            if sender.nav_until().is_some() {
                continue;
            }
            let Some(channel) = sender.channel() else {
                continue;
            };
            for (queue, attempt) in sender.published() {
                carried = true;
                let Some(first) = attempt.frames.first() else {
                    sender.complete_with(queue, ModelOutcome::Fail(TxStatus::Aborted));
                    continue;
                };
                let receivers = self
                    .radios
                    .iter()
                    .enumerate()
                    .filter(|(other, radio)| {
                        *other != index
                            && radio
                                .channel()
                                .is_some_and(|heard| same_primary(heard, channel))
                    })
                    .map(|(_, radio)| *radio);
                let address1 = address(first, 4);
                let mut acknowledged = false;
                for receiver in receivers {
                    if first[0] == 0xc4
                        && !address1.is_some_and(|address| has_interface(receiver, address))
                    {
                        let duration = u16::from_le_bytes([first[2], first[3]]);
                        receiver.reserve_medium(
                            receiver.channel().expect("the receiver is tuned"),
                            duration,
                        );
                    }
                    for frame in &attempt.frames {
                        if let Some(meta) = arrival(
                            receiver,
                            frame,
                            receiver.channel().expect("the receiver is tuned"),
                        ) {
                            receiver.receive(frame, meta);
                        }
                    }
                    acknowledged |=
                        address1.is_some_and(|address1| has_interface(receiver, address1));
                }
                let control = (first[0] >> 2) & 0b11 == 1;
                let individual = address1.is_some_and(|address1| address1[0] & 1 == 0);
                let outcome = if control || !individual || acknowledged {
                    ModelOutcome::Success
                } else {
                    ModelOutcome::Fail(TxStatus::AckTimeout)
                };
                sender.complete_with(queue, outcome);
            }
        }
        carried
    }
}

/// Whether a radio tuned to `heard` hears a PPDU sent on `sent`: the same
/// primary channel of the same band.
fn same_primary(heard: Channel, sent: Channel) -> bool {
    heard.band() == sent.band() && heard.number() == sent.number()
}

fn address(frame: &[u8], offset: usize) -> Option<[u8; 6]> {
    frame.get(offset..offset + 6)?.try_into().ok()
}

/// Whether `radio` has an interface of `address`.
fn has_interface<O: TxBody>(radio: &LowerMacModel<O>, address: [u8; 6]) -> bool {
    (0..2).any(|vif| {
        radio
            .vif_config(VifId(vif))
            .is_some_and(|config| config.address == address)
    })
}

/// The metadata `frame` arrives with at `receiver`, or `None` when the
/// receiver holds no key that opens it.
fn arrival<O: TxBody>(
    receiver: &LowerMacModel<O>,
    frame: &[u8],
    channel: Channel,
) -> Option<RxMeta> {
    let protected = frame.get(1).is_some_and(|flags| flags & 0x40 != 0);
    let crypto = if protected {
        let keys = receiver.installed_keys();
        let group = address(frame, 4).is_some_and(|address1| address1[0] & 1 != 0);
        let opened = match (group, address(frame, 10)) {
            (true, _) => keys
                .iter()
                .any(|scope| matches!(scope, KeyScope::Group { .. })),
            (false, Some(transmitter)) => keys.contains(&KeyScope::Pairwise { peer: transmitter }),
            (false, None) => false,
        };
        if !opened {
            return None;
        }
        RxCryptoStatus::DecryptedAndIntegrityVerified
    } else {
        RxCryptoStatus::Unprotected
    };
    let mut meta = RxMeta::unavailable(channel);
    meta.rssi_dbm = RxEvidence::HardwareObserved(-40);
    meta.noise_floor_dbm = RxEvidence::HardwareObserved(-96);
    meta.crypto = RxEvidence::HardwareObserved(crypto);
    Some(meta)
}
