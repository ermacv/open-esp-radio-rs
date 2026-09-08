//! Shared, versioned boundary between the unprivileged runner and finite helper.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Adapter(pub(crate) u16);

impl std::str::FromStr for Adapter {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || "adapter must be hci0 through hci65534 (canonical decimal)".to_owned();
        let number = value.strip_prefix("hci").ok_or_else(invalid)?;
        let index = number.parse::<u16>().map_err(|_| invalid())?;
        if index == u16::MAX || number != index.to_string() {
            return Err(invalid());
        }
        Ok(Self(index))
    }
}

impl std::fmt::Display for Adapter {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "hci{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    pub(crate) schema: u32,
    pub(crate) adapter: String,
    pub(crate) address: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) initial_powered: Option<bool>,
    pub(crate) initial_soft_blocked: Option<bool>,
    pub(crate) dtm_v2_advertised: bool,
    pub(crate) rx_started: bool,
    pub(crate) rx_packets: Option<u16>,
    pub(crate) tx_started: bool,
    pub(crate) tx_test_end: bool,
    pub(crate) restored: bool,
    pub(crate) errors: Vec<String>,
}

impl Check {
    pub(crate) fn new(adapter: Adapter) -> Self {
        Self {
            schema: 1,
            adapter: adapter.to_string(),
            address: None,
            version: None,
            initial_powered: None,
            initial_soft_blocked: None,
            dtm_v2_advertised: false,
            rx_started: false,
            rx_packets: None,
            tx_started: false,
            tx_test_end: false,
            restored: false,
            errors: Vec::new(),
        }
    }

    /// This proves command acceptance only; packet reception needs an RF peer.
    pub(crate) fn passed(&self, adapter: Adapter) -> bool {
        self.schema == 1
            && self.adapter == adapter.to_string()
            && self.address.is_some()
            && self.version.is_some()
            && self.initial_powered.is_some()
            && self.initial_soft_blocked.is_some()
            && self.dtm_v2_advertised
            && self.rx_started
            && self.rx_packets.is_some()
            && self.tx_started
            && self.tx_test_end
            && self.restored
            && self.errors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_rejects_paths_options_and_reserved_index() {
        for value in [
            "../hci0", "--help", "hci-1", "hci65535", "hci01", "hci+1", "hci",
        ] {
            assert!(value.parse::<Adapter>().is_err(), "{value}");
        }
        assert_eq!("hci0".parse::<Adapter>().unwrap(), Adapter(0));
        assert_eq!("hci65534".parse::<Adapter>().unwrap(), Adapter(65534));
    }
    #[test]
    fn mask_alone_and_incomplete_cleanup_cannot_pass() {
        let mut report = Check::new(Adapter(0));
        report.dtm_v2_advertised = true;
        assert!(!report.passed(Adapter(0)));
        report.address = Some("test".into());
        report.version = Some("test".into());
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(true);
        report.rx_started = true;
        report.rx_packets = Some(0);
        report.tx_started = true;
        report.tx_test_end = true;
        assert!(!report.passed(Adapter(0)));
        report.restored = true;
        assert!(report.passed(Adapter(0)));
        assert!(!report.passed(Adapter(1)));
        report.schema = 2;
        assert!(!report.passed(Adapter(0)));
        report.schema = 1;
        report.errors.push("reset failed".into());
        assert!(!report.passed(Adapter(0)));
    }
}
