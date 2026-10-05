//! The port station's coexistence over the S31 radio system.
//!
//! The coexistence schedule belongs to the radio system, not the Wi-Fi MAC:
//! the port station reads it and asks for its effects through
//! `oer-ieee80211-sta-service`'s `PortCoexistence`, which this serves with
//! what the direct station's power agent uses — the schedule's view from
//! [`WifiCoexViewCell`](oer_esp32s31_radio_runtime::WifiCoexViewCell),
//! [`apply_coex_action`] under the radio's lease, and the vendor reconnect
//! policy's per-frame requests ([`RadioConnectionCoex`]).

use oer_esp32s31_coex::CoexError;
use oer_esp32s31_hal::shared_radio::PlatformClockProvider;
use oer_esp32s31_ieee80211_sta::connection_coex::{ConnectionFrame, ConnectionFrameCoex};
use oer_esp32s31_radio_runtime::RadioSystem;
use oer_ieee80211_sta::modem_sleep::{CoexView, PmCoexAction};
use oer_ieee80211_sta_service::port::{
    PortCoexistence, PortCoexistenceRefused, PortConnectionFrame,
};

use super::power::{PowerCoexSource, apply_coex_action};
use crate::roles::radio_channel::RadioConnectionCoex;

/// The port station's [`PortCoexistence`] over `radio`.
pub struct Esp32s31PortCoexistence<'radio, P, C, T> {
    radio: &'radio RadioSystem<P, C, T>,
    /// Why the radio system refused the last effect it refused.
    refused: Option<CoexError>,
}

impl<'radio, P, C, T> Esp32s31PortCoexistence<'radio, P, C, T> {
    pub const fn new(radio: &'radio RadioSystem<P, C, T>) -> Self {
        Self {
            radio,
            refused: None,
        }
    }

    /// Why the radio system refused the last effect it refused.
    pub const fn refused(&self) -> Option<CoexError> {
        self.refused
    }
}

impl<P, C: PlatformClockProvider, T: oer_time::Timer> PortCoexistence
    for Esp32s31PortCoexistence<'_, P, C, T>
{
    fn view(&self) -> CoexView {
        self.radio.wifi_coex_view().power_coex().view
    }

    async fn perform(&mut self, action: PmCoexAction) -> Result<(), PortCoexistenceRefused> {
        let mut radio = self.radio.lock().await;
        apply_coex_action(&mut radio, action).map_err(|error| {
            self.refused = Some(error);
            PortCoexistenceRefused
        })
    }

    async fn connection_frame(&mut self, frame: PortConnectionFrame) -> bool {
        let frame = match frame {
            PortConnectionFrame::ProbeRequest => ConnectionFrame::ProbeRequest,
            PortConnectionFrame::Authentication => ConnectionFrame::Authentication,
            PortConnectionFrame::Association => ConnectionFrame::Association,
            PortConnectionFrame::Eapol => ConnectionFrame::Eapol,
        };
        // The S31 port turns `CoexPriority::Elevated` into the reconnect
        // priorities the radio system then reads for the frame.
        RadioConnectionCoex::new(self.radio)
            .connection_frame(frame)
            .await
            .is_some()
    }
}
