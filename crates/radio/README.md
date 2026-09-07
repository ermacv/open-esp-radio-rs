# Radio control and lifecycle

`oer-radio` exposes Wi-Fi configuration, requests and affine role
lifecycle through `wifi`. Its contracts do not acquire PAC, DMA or interrupt
owners. Concrete integration supplies the service profile and owns hardware.

`runtime::embassy`, enabled by `wifi-embassy`, transports commands and drives
the complete local role epoch. A controlled child must return its exact owner
before stop completion; failure retains the faulted owner. Executor bindings
depend on the public contracts within this crate, avoiding a dependency cycle
with the generic Embassy Wi-Fi service crate.

Internal components use `oer_radio::wifi` and
`oer_radio::runtime::embassy`. Applications use the separate
[public facade](../oer/README.md), which exposes these same types without
owning or duplicating their state.

Tests use synthetic portable service profiles in `wifi/test_support.rs`.
They do not depend on a concrete chip profile. Run both the default profile
and `cargo test -p oer-radio --all-features` to exercise the optional
Embassy supervisor's ownership, cancellation and stop contracts.

See the [driver architecture](../README.md) for the complete ownership graph.
