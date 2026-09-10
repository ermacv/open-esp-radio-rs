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

/// One public Bluetooth peer, supplied as six hexadecimal octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeerAddress(pub(crate) [u8; 6]);

impl std::str::FromStr for PeerAddress {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || "peer must be a public address such as 30:ED:A0:F3:F6:D1".to_owned();
        let octets: Vec<_> = value.split(':').collect();
        if octets.len() != 6 {
            return Err(invalid());
        }
        let mut wire = [0; 6];
        for (index, octet) in octets.into_iter().enumerate() {
            if octet.len() != 2 || !octet.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(invalid());
            }
            wire[5 - index] = u8::from_str_radix(octet, 16).map_err(|_| invalid())?;
        }
        Ok(Self(wire))
    }
}

impl std::fmt::Display for PeerAddress {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, byte) in self.0.iter().rev().enumerate() {
            if index != 0 {
                output.write_str(":")?;
            }
            write!(output, "{byte:02X}")?;
        }
        Ok(())
    }
}

/// Command-level evidence; Reset timing does not prove an on-air loss boundary.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionReset {
    pub(crate) schema: u32,
    pub(crate) adapter: String,
    pub(crate) peer: String,
    pub(crate) hold_ms: u16,
    pub(crate) initial_powered: Option<bool>,
    pub(crate) initial_soft_blocked: Option<bool>,
    pub(crate) connection_complete: bool,
    pub(crate) reset_completed: bool,
    pub(crate) connection_after_micros: Option<u64>,
    pub(crate) reset_after_connection_micros: Option<u64>,
    pub(crate) restored: bool,
    pub(crate) errors: Vec<String>,
}

impl ConnectionReset {
    pub(crate) fn new(adapter: Adapter, peer: PeerAddress, hold_ms: u16) -> Self {
        Self {
            schema: 1,
            adapter: adapter.to_string(),
            peer: peer.to_string(),
            hold_ms,
            initial_powered: None,
            initial_soft_blocked: None,
            connection_complete: false,
            reset_completed: false,
            connection_after_micros: None,
            reset_after_connection_micros: None,
            restored: false,
            errors: Vec::new(),
        }
    }

    pub(crate) fn passed(&self, adapter: Adapter, peer: PeerAddress, hold_ms: u16) -> bool {
        self.schema == 1
            && self.adapter == adapter.to_string()
            && self.peer == peer.to_string()
            && self.hold_ms == hold_ms
            && hold_ms <= 5_000
            && self.initial_powered.is_some()
            && self.initial_soft_blocked.is_some()
            && self.connection_complete
            && self.reset_completed
            && self.connection_after_micros.is_some()
            && self.reset_after_connection_micros.is_some()
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

#[cfg(test)]
mod connection_reset_tests {
    use super::*;
    #[test]
    fn reset_report_requires_the_exact_request_and_complete_restoration() {
        let adapter = Adapter(0);
        let peer: PeerAddress = "30:ed:a0:f3:f6:d1".parse().unwrap();
        assert_eq!(peer.to_string(), "30:ED:A0:F3:F6:D1");
        let mut report = ConnectionReset::new(adapter, peer, 0);
        assert!(!report.passed(adapter, peer, 0));
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(true);
        report.connection_complete = true;
        report.connection_after_micros = Some(100);
        report.reset_after_connection_micros = Some(1);
        report.reset_completed = true;
        assert!(!report.passed(adapter, peer, 0));
        report.restored = true;
        assert!(report.passed(adapter, peer, 0));
        assert!(!report.passed(adapter, peer, 1));
        assert!(!report.passed(Adapter(1), peer, 0));
        assert!(!report.passed(adapter, "30:ED:A0:F3:F6:D2".parse().unwrap(), 0));
        report.errors.push("Reset failed".into());
        assert!(!report.passed(adapter, peer, 0));
    }
}
