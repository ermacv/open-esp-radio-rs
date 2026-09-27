//! Coexistence hardware and core leaves of the pinned `libcoexist.a`,
//! compared with the compiled production coexistence probes on the
//! `wifi-mac` leaf machinery.
use crate::mac::{Domain, Leaf, Suite, leaf, released, released_when_leased};

/// Hardware timers the production bank admits (`CoexTimerIndex`).
const TIMERS: &[u32] = &[0, 1, 2, 3, 4];

/// Coexistence events production admits (`CoexEventId`: below
/// `COEX_EVENT_COUNT`, 49).
const EVENTS: &[u32] = &events::<49>(None);
/// The admitted events and the first event beyond them.
const EVENTS_AND_INVALID: &[u32] = &events::<50>(None);
/// The PHY grant-protect event: vendor `coex_core_timer_idx_get` maps it to
/// timer 5, which production reserves for the radio arbiter's
/// `oer_esp32s31_hal::coex::PhyGrantProtect` instead of the driver's policy
/// timers (reviewed decision with the PHY owner, 2026-09-27). The driver
/// leaves compare every other event; the grant-protect leaves compare the
/// event through its own owner.
const GRANT_PROTECT_EVENT: u32 = 48;
/// The driver's events and the first event beyond them.
const DRIVER_EVENTS_AND_INVALID: &[u32] = &events::<49>(Some(GRANT_PROTECT_EVENT));

/// Events `0..` in order, skipping `excluded`.
const fn events<const N: usize>(excluded: Option<u32>) -> [u32; N] {
    let mut values = [0; N];
    let (mut index, mut event) = (0, 0);
    while index < N {
        if !matches!(excluded, Some(e) if e == event) {
            values[index] = event;
            index += 1;
        }
        event += 1;
    }
    values
}

/// A nullable output object of `length` bytes.
const fn output(length: u32) -> Domain {
    Domain::Output {
        offset: 0,
        length,
        nullable: true,
    }
}

/// Production borrows the timer bank through a validation lease of the
/// radio arbiter and releases it with one release fence after the leaf's
/// register transaction.
const RELEASE_FENCES: u32 = 1;

/// Every compared coexistence leaf.
pub const LEAVES: &[Leaf] = &[
    released(
        leaf(
            "coex_hw_timer_enable",
            "open_coex_trace_coex_hw_timer_enable",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_disable",
            "open_coex_trace_coex_hw_timer_disable",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_force",
            "open_coex_trace_coex_hw_timer_force",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    released(
        leaf(
            "coex_hw_timer_unforce",
            "open_coex_trace_coex_hw_timer_unforce",
            &[("index", Domain::Words(TIMERS))],
            false,
        ),
        RELEASE_FENCES,
    ),
    // The vendor reads its priority table without a bound, so the domain
    // is the admitted events.
    leaf(
        "coex_core_pti_get",
        "open_coex_core_trace_pti_get",
        &[("event", Domain::Words(EVENTS)), ("output", output(1))],
        true,
    ),
    leaf(
        "coex_core_event_duration_get",
        "open_coex_core_trace_event_duration_get",
        &[
            ("event", Domain::Words(EVENTS_AND_INVALID)),
            ("output", output(4)),
        ],
        true,
    ),
    leaf(
        "coex_core_timer_idx_get",
        "open_coex_core_trace_timer_idx_get",
        &[("event", Domain::Words(DRIVER_EVENTS_AND_INVALID))],
        true,
    ),
    // An event without a policy timer is rejected before the lease.
    released_when_leased(
        leaf(
            "coex_core_release",
            "open_coex_core_trace_release",
            &[
                ("_client", Domain::Words(&[0, 1])),
                ("event", Domain::Words(DRIVER_EVENTS_AND_INVALID)),
            ],
            true,
        ),
        RELEASE_FENCES,
    ),
];

/// The coexistence hardware suite.
pub const COEX_HW: Suite = Suite {
    title: "Coexistence hardware leaf comparison",
    archives: &["libcoexist"],
    leaves: LEAVES,
    wifi: false,
    absent: &[],
};
