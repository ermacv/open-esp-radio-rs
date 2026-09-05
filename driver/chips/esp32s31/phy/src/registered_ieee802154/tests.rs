use open_esp_radio_esp32s31_hal::{
    Ieee802154MacPolicy, Ieee802154TimingReady as HalIeee802154TimingReady,
};

use super::{
    RegisteredIeee802154FoundationTransitionFailure, RegisteredIeee802154MacPolicyConfigured,
    RegisteredIeee802154MacPolicyRecovery, RegisteredIeee802154MacPolicyTransitionFailure,
    RegisteredIeee802154Prerequisites, RegisteredIeee802154Reset, RegisteredIeee802154TimingReady,
    map_prerequisites, preserve_prerequisites,
};
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    state::client::{
        DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState, PhyModemClient, PhyPllTrackClock,
    },
};

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered_state() -> RegisteredPhyState {
    RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production()))
}

fn model_prerequisites() -> RegisteredIeee802154Prerequisites {
    let registered = registered_state();
    let gain_parameter = registered.state().register_init_parameters().parameter_120;
    let clients = PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS)
        .acquire(PhyModemClient::Ieee802154, &mut FixedClock(0))
        .unwrap_or_else(|_| panic!("model client acquisition must succeed"))
        .into_owner()
        .unwrap_or_else(|_| panic!("fresh model timestamp must settle"));
    RegisteredIeee802154Prerequisites {
        registered,
        clients,
        timing: HalIeee802154TimingReady::for_host_ownership_model(gain_parameter),
    }
}

#[test]
fn production_prerequisite_combinators_cover_success_failure_and_recovery_moves() {
    let (after, prerequisites) =
        match preserve_prerequisites(41_u8, model_prerequisites(), |before| {
            Ok::<_, &'static str>(before + 1)
        }) {
            Ok(success) => success,
            Err(_) => panic!("success branch must retain all prerequisites"),
        };
    assert_eq!(after, 42);
    assert!(prerequisites.phy_state().phy_registered());
    assert_eq!(
        prerequisites.timing_gain_parameter(),
        prerequisites
            .phy_state()
            .register_init_parameters()
            .parameter_120
    );

    let (failure, prerequisites) = match preserve_prerequisites(7_u8, model_prerequisites(), |_| {
        Err::<u8, _>("typed failure")
    }) {
        Ok(_) => panic!("failure branch must retain all prerequisites"),
        Err(failure) => failure,
    };
    assert_eq!(failure, "typed failure");
    assert!(prerequisites.phy_state().phy_registered());

    let (after, prerequisites) =
        map_prerequisites(3_u8, model_prerequisites(), |before| before + 2);
    assert_eq!(after, 5);
    assert!(prerequisites.phy_state().phy_registered());
}

#[test]
fn registered_full_chain_and_typed_recovery_surface_connects() {
    fn full_success_chain<P>(
        owner: RegisteredIeee802154TimingReady<P>,
        policy: Ieee802154MacPolicy,
    ) -> Option<RegisteredIeee802154MacPolicyConfigured<P>> {
        let reset = owner.reset_mac().ok()?;
        let foundation = reset.configure_foundation().ok()?;
        foundation.configure_mac_policy(policy).ok()
    }

    fn recover_foundation<P>(
        failure: RegisteredIeee802154FoundationTransitionFailure<P>,
    ) -> RegisteredIeee802154Reset<P> {
        failure.into_reset()
    }

    fn recover_policy<P>(
        failure: RegisteredIeee802154MacPolicyTransitionFailure<P>,
    ) -> RegisteredIeee802154MacPolicyRecovery<P> {
        failure.into_recovery()
    }

    let _ = full_success_chain::<()>;
    let _ = recover_foundation::<()>;
    let _ = recover_policy::<()>;
}
