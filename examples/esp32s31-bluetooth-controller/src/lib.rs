#![no_std]
#![forbid(unsafe_code)]

//! Application configuration for the bounded Controller smoke sequences.

use bt_hci::{
    cmd::le::LeSetAdvParams,
    param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration},
};

/// Legacy advertising cases supported by the current S31 event graphs.
#[derive(Clone, Copy)]
pub enum AdvertisingSmokeCase {
    Nonconnectable,
    Connectable,
}

impl AdvertisingSmokeCase {
    pub const ALL: [Self; 2] = [Self::Nonconnectable, Self::Connectable];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Nonconnectable => "nonconnectable",
            Self::Connectable => "connectable",
        }
    }

    /// Build the typed command used for initial enable and reconfiguration.
    pub fn parameters(self) -> LeSetAdvParams {
        let (kind, channels) = match self {
            Self::Nonconnectable => (AdvKind::AdvNonconnInd, AdvChannelMap::ALL),
            // The response-capable S31 graph owns exactly one primary channel.
            Self::Connectable => (AdvKind::AdvInd, AdvChannelMap::CHANNEL_37),
        };
        LeSetAdvParams::new(
            Duration::from_millis(100),
            Duration::from_millis(100),
            kind,
            AddrKind::RANDOM,
            AddrKind::PUBLIC,
            BdAddr::default(),
            channels,
            AdvFilterPolicy::Unfiltered,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::cmd::Cmd;

    #[test]
    fn smoke_commands_fit_their_radio_role_channel_capacity() {
        for case in AdvertisingSmokeCase::ALL {
            let command = case.parameters();
            let params = command.params();
            let channels = params.adv_channel_map;
            let selected = [
                channels.is_channel_37_enabled(),
                channels.is_channel_38_enabled(),
                channels.is_channel_39_enabled(),
            ]
            .into_iter()
            .filter(|enabled| *enabled)
            .count();
            match params.adv_kind {
                AdvKind::AdvInd => assert_eq!(selected, 1, "response graph has one channel"),
                AdvKind::AdvNonconnInd => assert_eq!(selected, 3, "exercise the full TX chain"),
                _ => panic!("the smoke selected an unsupported advertising role"),
            }
        }
    }
}
