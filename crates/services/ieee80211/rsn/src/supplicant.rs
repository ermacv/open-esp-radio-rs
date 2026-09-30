//! Asynchronous key-data unwrap around the sans-IO station supplicant.

use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk,
    aes::AsyncRsnKeyUnwrap,
    supplicant::{RsnStaProcessError, RsnStaSupplicant, RsnStaSupplicantAction},
};

/// Consume one peer EAPOL-Key frame and resolve a requested key-data unwrap
/// through `unwrap`.
///
/// The result never carries [`RsnStaSupplicantAction::UnwrapKeyData`]: an
/// encrypted Message 3 is unwrapped here and completed into
/// [`RsnStaSupplicantAction::InstallKeys`], and an unwrap failure is
/// [`RsnStaProcessError::KeyUnwrap`] after the supplicant has rejected the key
/// data.
pub async fn process_frame<const N: usize, U: AsyncRsnKeyUnwrap>(
    supplicant: &mut RsnStaSupplicant,
    frame: OwnedEapolFrame<N>,
    pmk: &Pmk,
    unwrap: &mut U,
) -> Result<RsnStaSupplicantAction<N>, RsnStaProcessError<U::Error>> {
    match supplicant.on_frame(frame, pmk)? {
        RsnStaSupplicantAction::UnwrapKeyData(request) => {
            let unwrapped = unwrap
                .unwrap_key_data(request.kek(), request.wrapped_key_data())
                .await;
            supplicant
                .complete_key_data_unwrap(request, unwrapped)
                .map(RsnStaSupplicantAction::InstallKeys)
        }
        action => Ok(action),
    }
}

#[cfg(test)]
mod tests;
