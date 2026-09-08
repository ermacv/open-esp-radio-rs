use super::*;

fn observation(phy: PhyExpectation) -> Observation {
    Observation {
        enabled: true,
        channel: 13,
        htmode: if phy == PhyExpectation::He20 {
            "HE20"
        } else {
            "HT20"
        }
        .into(),
        geometry: "Interface phy0-ap0\n\tchannel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz\n"
            .into(),
        ht: true,
        he: phy == PhyExpectation::He20,
    }
}

#[test]
fn ht20_width_does_not_prove_he20() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::He20,
        channel: 13,
    };
    let mut observed = observation(PhyExpectation::Ht20);
    assert!(profile.verify(&observed).is_err());
    observed.htmode = "HE20".into();
    assert!(
        profile.verify(&observed).is_err(),
        "UCI intent alone is insufficient"
    );
    observed.he = true;
    profile.verify(&observed).unwrap();
    observed.channel = 11;
    assert!(profile.verify(&observed).is_err());
}

#[test]
fn ht40_requires_the_requested_secondary_channel() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::Ht40,
        channel: 13,
    };
    let mut observed = observation(PhyExpectation::Ht20);
    observed.htmode = "HT40-".into();
    observed.geometry = "channel 13 (2472 MHz), width: 40 MHz, center1: 2482 MHz".into();
    assert!(profile.verify(&observed).is_err());
    observed.geometry = "channel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz".into();
    profile.verify(&observed).unwrap();
}

#[test]
fn capability_is_interface_specific_and_respects_regulation() {
    let profile = Profile {
        ht40_above: false,
        phy: PhyExpectation::He20,
        channel: 13,
    };
    let caps = "Supported interface modes:\n * AP\n HT20/HT40\n HE Iftypes: managed\n * 2472 MHz [13] (20.0 dBm)\n";
    assert!(verify_capabilities(profile, caps).is_err());
    let caps = caps.replace("HE Iftypes: managed", "HE Iftypes: AP");
    verify_capabilities(profile, &caps).unwrap();
    assert!(verify_capabilities(profile, &caps.replace("(20.0 dBm)", "(disabled)")).is_err());
    assert!(
        verify_capabilities(profile, &caps.replace("(20.0 dBm)", "(20.0 dBm) (no IR)")).is_err()
    );
}

#[derive(Clone)]
struct Fake {
    calls: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    fail: &'static str,
    before: Value,
}

impl Backend for Fake {
    fn invoke(
        &self,
        operation: &str,
        _options: &Value,
        up: bool,
        _pending: &Value,
        _ap_enabled: Option<bool>,
    ) -> Result<Zeroizing<String>> {
        self.calls.borrow_mut().push(format!("{operation}:{up}"));
        if operation == self.fail {
            return Err("injected failure after remote mutation".into());
        }
        if operation == "restore" && self.fail == "ap-state" {
            let mut after = self.before.clone();
            after["ap_enabled"] = json!(false);
            return Ok(Zeroizing::new(after.to_string()));
        }
        Ok(Zeroizing::new(self.before.to_string()))
    }
    fn observe(&self) -> Result<Observation> {
        if self.fail == "observe" {
            return Err("injected missing hostapd".into());
        }
        Ok(observation(PhyExpectation::He20))
    }
}

#[test]
fn restoring_uci_and_radio_up_is_insufficient_if_the_original_ap_is_missing() {
    let fake = Fake {
        calls: Default::default(),
        fail: "ap-state",
        before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
    };
    let mut owner = AccessPoint::prepare(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
        },
        json!({}),
    )
    .unwrap();
    assert!(owner.restore().is_err());
    assert!(!owner.restored);
    owner.backend.fail = "";
    owner.restore().unwrap();
    assert!(owner.restored);
}

#[test]
fn restores_after_partial_apply_readback_failure_and_stop_failure() {
    for fail in ["apply", "observe", "state", ""] {
        let calls = Default::default();
        let fake = Fake {
            calls,
            fail,
            before: json!({"up": false, "ap_enabled": false, "options": {"radio0": {"htmode": "HT40"}}, "pending": {"radio0": {"htmode": true}}}),
        };
        let observed = fake.calls.clone();
        let owner = AccessPoint::prepare(
            fake,
            Profile {
                ht40_above: false,
                phy: PhyExpectation::He20,
                channel: 13,
            },
            json!({}),
        );
        if let Ok(mut owner) = owner {
            let _ = owner.stop();
        }
        let calls = observed.borrow();
        assert_eq!(calls.first().unwrap(), "snapshot:true");
        assert_eq!(
            calls.last().unwrap(),
            "restore:false",
            "restore initial down state after {fail}"
        );
        assert_eq!(
            calls.iter().filter(|call| *call == "restore:false").count(),
            1
        );
    }
}

#[test]
fn restore_failure_is_not_a_successful_owner_release() {
    let directory = tempfile::tempdir().unwrap();
    let scope = super::super::cleanup::Scope::new(directory.path());
    let fake = Fake {
        calls: Default::default(),
        fail: "restore",
        before: json!({"up": true, "ap_enabled": true, "options": {}, "pending": {}}),
    };
    let mut owner = AccessPoint::prepare(
        fake,
        Profile {
            ht40_above: false,
            phy: PhyExpectation::He20,
            channel: 13,
        },
        json!({}),
    )
    .unwrap();
    assert!(owner.restore().is_err());
    assert!(!owner.restored);
    drop(owner);
    assert!(
        scope
            .finish()
            .unwrap()
            .iter()
            .any(|record| record.failure.is_some())
    );
    assert!(super::super::cleanup::require_healthy().is_err());
}
