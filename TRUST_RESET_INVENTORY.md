# Workbench trust-reset proof inventory

Status: **Stage 1 audit complete; no path is trusted by the new proof model**.

The pre-reset workspace is archived as
`archive/pre-workbench-trust-reset-20260813` at
`4aa86a7155317688beec667e884224d523c4415a` (tree
`50aadd5f431b834702e831a6d9f2eec49ae67968`).

## Meaning of this inventory

`classification != trust`.

- `ACCEPT_ATTEST` means only that the path is structurally eligible for
  Stage-2 attestation. It does not mean trusted, proven, qualified or `MATCH`.
- `ACCEPT_REWRITE` retains a useful claim whose present proof path violates
  the new trust boundary.
- `ACCEPT_QUARANTINE` retains analysis or regression value but excludes the
  current binding from production proof.
- Every record has `trust: false`. Old green results are not grandfathered.

Stage 1 records source-level reachability only. Stage 2 must replace it with
compiled-artifact provenance and reachability relative to the named shipping
profile. Concrete replay must separately demonstrate scenario execution
coverage. These are three different properties.

An absent fact is written as `none`; no shipping root is invented to fill a
field. `current_consumer` describes legacy wiring and grants no authority.
Only `allowed_consumer` describes permitted use under the new model.

Every record uses the same fields: `id`, `proof_kind`, `classification`,
`purpose`, `trust`, `vendor_target`, `rust_target`, `shipping_profile`,
`shipping_root`, `source_production_call_path`, `oracle_kind`,
`terminal_match_capable`, `required_for_terminal_match`, `current_consumer`,
`allowed_consumer`, `legacy_status` and `reason`.

## Effect/rust-probe paths

### E01 — Bluetooth coexistence PTI

- `id: E01`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: btbb::coex_pti_v2`; `rust_target: hal::coex::configure_bluetooth_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature internal-ble-coex-client-boundary`; `allowed_consumer: analysis/regression-only`; `legacy_status: release-required Workbench feature, not production proof`
- `reason: the HAL leaf is called only by the verification probe; no shipping runtime caller exists`

### E02 — COEX request

- `id: E02`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_core_request`; `rust_target: CoexCore::request_wifi/request_bluetooth`
- `shipping_profile: esp32s31/coex/embassy-adapter`; `shipping_root: CoexOwner::run`; `source_production_call_path: CoexOwner::run -> CoexCore::request_wifi/request_bluetooth -> CoexCore::request -> program_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: separate setup, clock environment, raw observations and generic verdict`
- `current_consumer: Workbench feature coex-request-valid-clock-projection`; `allowed_consumer: none until rewrite`; `legacy_status: provider-controlled projection`
- `reason: the probe enables a fresh core, maps policy and supplies an MMIO-capable CoexProbeClock`

### E03 — COEX release

- `id: E03`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_core_release`; `rust_target: CoexCore::release`
- `shipping_profile: esp32s31/coex/embassy-adapter`; `shipping_root: CoexOwner::run`; `source_production_call_path: CoexOwner::run -> CoexCore::release -> CoexTimerHardware::disable -> CoexTimerHal::disable -> RadioRegisters::disable_coex_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: future COEX qualification`; `legacy_status: untrusted candidate`
- `reason: setup and return encoding can remain outside a target window around the real production call`

### E04 — COEX scheduler interval set

- `id: E04`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: coex::coex_schm_interval_set`; `rust_target: CoexScheduler::set_interval`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature internal-coex-scheduler-boundary`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: CoexScheduler is currently used only by probes and tests`

### E05 — COEX scheduler interval get

- `id: E05`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: coex::coex_schm_interval_get`; `rust_target: CoexScheduler::interval`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature internal-coex-scheduler-boundary`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: CoexScheduler is currently used only by probes and tests`

### E06 — COEX scheduler current period

- `id: E06`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: coex::coex_schm_curr_period_get`; `rust_target: CoexScheduler::current_period`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature internal-coex-scheduler-boundary`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the probe constructs the only active schedule and no shipping runtime owns this scheduler`

### E07 — COEX scheduler current phase

- `id: E07`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: coex::coex_schm_curr_phase_get`; `rust_target: CoexScheduler::current_phase`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and comparison of production output rather than probe address synthesis`
- `current_consumer: Workbench feature internal-coex-scheduler-boundary`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: no shipping caller exists and the probe synthesizes a vendor-layout pointer from the result`

### E08 — COEX timer programming

- `id: E08`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_hw_timer_set`; `rust_target: coex::program_timer`
- `shipping_profile: esp32s31/coex/embassy-adapter`; `shipping_root: CoexOwner::run`; `source_production_call_path: CoexOwner::run -> CoexCore::request -> program_timer -> CoexTimerHal`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: executor-recorded clock interaction and environment-owned responses`
- `current_consumer: Workbench feature coex-timer-valid-bank-programming`; `allowed_consumer: none until rewrite`; `legacy_status: environment mixed into probe execution`
- `reason: CoexProbeClock owns behavior and raw MMIO reads inside the compared path`

### E09 — COEX timer enable

- `id: E09`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_hw_timer_enable`; `rust_target: RadioRegisters::enable_coex_timer`
- `shipping_profile: esp32s31/coex/embassy-adapter`; `shipping_root: CoexOwner::run`; `source_production_call_path: CoexOwner::run -> CoexCore::request -> CoexTimerHardware::enable -> CoexTimerHal::enable -> RadioRegisters::enable_coex_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: Workbench feature coex-hardware-timer-control`; `allowed_consumer: future COEX qualification`; `legacy_status: untrusted candidate`
- `reason: thin typed PAC leaf on the production request path`

### E10 — COEX timer disable

- `id: E10`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_hw_timer_disable`; `rust_target: RadioRegisters::disable_coex_timer`
- `shipping_profile: esp32s31/coex/embassy-adapter`; `shipping_root: CoexOwner::run`; `source_production_call_path: CoexOwner::run -> CoexCore::release/disable -> CoexTimerHardware::disable -> CoexTimerHal::disable -> RadioRegisters::disable_coex_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: Workbench feature coex-hardware-timer-control`; `allowed_consumer: future COEX qualification`; `legacy_status: untrusted candidate`
- `reason: thin typed PAC leaf on production release and shutdown paths`

### E11 — COEX timer force

- `id: E11`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_hw_timer_force`; `rust_target: RadioRegisters::force_coex_timer`
- `shipping_profile: esp32s31/coex/hal-api`; `shipping_root: CoexTimerHardware::force`; `source_production_call_path: CoexTimerHardware::force -> CoexTimerHal::force -> RadioRegisters::force_coex_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and declared HAL profile`
- `current_consumer: Workbench feature coex-hardware-timer-control`; `allowed_consumer: future COEX qualification`; `legacy_status: untrusted candidate`
- `reason: the checked implementation is the closed production HAL operation; runtime integration remains a separate capability concern`

### E12 — COEX timer unforce

- `id: E12`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: coex::coex_hw_timer_unforce`; `rust_target: RadioRegisters::unforce_coex_timer`
- `shipping_profile: esp32s31/coex/hal-api`; `shipping_root: CoexTimerHardware::unforce`; `source_production_call_path: CoexTimerHardware::unforce -> CoexTimerHal::unforce -> RadioRegisters::unforce_coex_timer`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and declared HAL profile`
- `current_consumer: Workbench feature coex-hardware-timer-control`; `allowed_consumer: future COEX qualification`; `legacy_status: untrusted candidate`
- `reason: the checked implementation is the closed production HAL operation; runtime integration remains a separate capability concern`

### E13 — STA join transition

- `id: E13`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: libnet80211::ieee80211_sta_new_state`; `rust_target: StaJoinRunner`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31StaAttemptTarget::authenticate/associate`; `source_production_call_path: Esp32s31StaAttemptTarget::authenticate/associate -> Esp32s31StaJoinPort -> StaJoinRunner::authenticate/associate`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: environment-owned RX/timer behavior and claim-owned projection`
- `current_consumer: Workbench feature wifi-sta-connected-no-power-save`; `allowed_consumer: none until rewrite`; `legacy_status: provider-controlled verdict`
- `reason: the probe embeds behavioral RX, timer, scenarios and result assertions around the real runner`

### E14 — MAC interrupt sample

- `id: E14`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_interrupt_get_event`; `rust_target: interrupt_snapshot::sample_mac_interrupt`
- `shipping_profile: esp32s31/wifi-sta/esp-hal`; `shipping_root: mac_interrupt`; `source_production_call_path: mac_interrupt -> service_mac_interrupt -> handle_mac_irq -> MacInterruptRegisters::mac_interrupt_status -> sample_mac_interrupt`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability interrupt-recovery after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: thin production PAC leaf with a real hard-IRQ caller`

### E15 — MAC interrupt acknowledge

- `id: E15`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_interrupt_clr_event`; `rust_target: interrupt_snapshot::acknowledge_mac_interrupt`
- `shipping_profile: esp32s31/wifi-sta/esp-hal`; `shipping_root: mac_interrupt`; `source_production_call_path: mac_interrupt -> service_mac_interrupt -> handle_mac_irq -> MacInterruptRegisters::acknowledge_mac_interrupts -> acknowledge_mac_interrupt`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability interrupt-recovery after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: thin production PAC leaf with a real hard-IRQ caller`

### E16 — Complete MAC FIQ slice

- `id: E16`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: negative_fixture`; `trust: false`
- `vendor_target: libpp::wDev_ProcessFiq`; `rust_target: irq::handle_mac_irq (declared partial target)`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: not applicable to this binding`
- `current_consumer: Workbench feature wifi-interrupt-runtime-primitive`; `allowed_consumer: analysis/regression-only`; `legacy_status: provider self-certification and legacy qualification input`
- `reason: the probe duplicates drain bounds, work ordering and semantic result encoding after calling production code`

### E17 — Beacon-miss timeout

- `id: E17`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_beacon_miss_timeout`; `rust_target: mac_modem_wakeup::set_beacon_miss_timeout`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E18 — Beacon-miss limit

- `id: E18`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_beacon_miss_limit`; `rust_target: mac_modem_wakeup::set_beacon_miss_limit`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E19 — Beacon-miss wake enable

- `id: E19`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable`; `rust_target: mac_modem_wakeup::enable_beacon_miss_limit_wakeup`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E20 — Modem sleep limit

- `id: E20`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_state_sleep_limit`; `rust_target: mac_modem_wakeup::set_modem_state_sleep_limit`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E21 — Modem sleep-limit wake enable

- `id: E21`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable`; `rust_target: mac_modem_wakeup::enable_modem_state_sleep_limit_wakeup`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E22 — Modem wakeup protection enable

- `id: E22`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_state_wakeup_protect_enable`; `rust_target: mac_modem_wakeup::enable_modem_state_wakeup_protect`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E23 — Modem wakeup lead time

- `id: E23`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_state_wakeup_protect_early_time`; `rust_target: mac_modem_wakeup::set_wakeup_protect_early_time`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E24 — TBTT auto-period enable

- `id: E24`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_tbtt_auto_period_enable`; `rust_target: mac_modem_wakeup::enable_tbtt_auto_period`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E25 — TBTT auto-period disable

- `id: E25`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_tbtt_auto_period_disable`; `rust_target: mac_modem_wakeup::disable_tbtt_auto_period`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E26 — TBTT auto-period interval

- `id: E26`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::pwr_hal_set_mac_modem_tbtt_auto_period_interval`; `rust_target: mac_modem_wakeup::set_tbtt_auto_period`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration and concrete-replay`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: lifted leaf only`
- `reason: the enclosing configure_station_modem_wakeup operation has no shipping caller`

### E27 — Power interrupt sample

- `id: E27`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_pwr_interrupt_get_event`; `rust_target: interrupt_snapshot::sample_mac_power_interrupt`
- `shipping_profile: esp32s31/wifi-sta/esp-hal`; `shipping_root: power_interrupt`; `source_production_call_path: power_interrupt -> service_power_interrupt -> handle_power_irq -> MacPowerInterruptRegisters::power_interrupt_status -> sample_mac_power_interrupt`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability interrupt-recovery after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: thin production PAC leaf with a real hard-IRQ caller`

### E28 — Power interrupt acknowledge

- `id: E28`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_pwr_interrupt_clr_event`; `rust_target: interrupt_snapshot::acknowledge_mac_power_interrupt`
- `shipping_profile: esp32s31/wifi-sta/esp-hal`; `shipping_root: power_interrupt`; `source_production_call_path: power_interrupt -> service_power_interrupt -> handle_power_irq -> MacPowerInterruptRegisters::acknowledge_power_interrupts -> acknowledge_mac_power_interrupt`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability interrupt-recovery after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: thin production PAC leaf with a real hard-IRQ caller`

### E29 — RX walker disable

- `id: E29`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_disable`; `rust_target: mac_rx_dma::set_walker_enabled(false)`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxDma::try_with_walker_stopped -> RadioRegisters::try_disable_mac_rx_walker -> set_walker_enabled(false)`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production RX shutdown reaches the exact PAC leaf`

### E30 — RX walker enable

- `id: E30`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_enable`; `rust_target: mac_rx_dma::set_walker_enabled(true)`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxDma::try_with_walker_enabled -> RadioRegisters::try_enable_mac_rx_walker -> set_walker_enabled(true)`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production RX startup reaches the exact PAC leaf`

### E31 — RX last descriptor read

- `id: E31`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_read_rxdscrlast`; `rust_target: mac_rx_dma::read_last_descriptor`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxRing -> RxDma::last_descriptor_low/with_ordered_cursor -> RadioRegisters::mac_rx_last_descriptor_low -> read_last_descriptor`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production ring observation reaches the exact PAC read`

### E32 — RX next descriptor read

- `id: E32`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_read_rxdscrnext`; `rust_target: mac_rx_dma::read_next_descriptor`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxRing -> RxDma::next_descriptor_low/with_ordered_cursor -> RadioRegisters::mac_rx_next_descriptor_low -> read_next_descriptor`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production ring observation reaches the exact PAC read`

### E33 — RX descriptor base write

- `id: E33`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_set_base`; `rust_target: mac_rx_dma::write_descriptor_base`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxRing::publish/repair -> RxDma::write_descriptor_base -> RadioRegisters::write_mac_rx_descriptor_base -> write_descriptor_base`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: retained DMA ownership guards a production write to the exact PAC leaf`

### E34 — RX complete last descriptor address

- `id: E34`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_get_last_dscr`; `rust_target: mac_rx_dma::read_last_descriptor_address`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxDma/RadioRegisters observation boundary -> RadioRegisters::mac_rx_last_descriptor_address -> read_last_descriptor_address`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: the production PAC exposes the complete vendor pointer reconstruction without probe decisions`

### E35 — RX descriptor reload query

- `id: E35`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_is_dscr_reload`; `rust_target: mac_rx_dma::descriptor_reload_pending`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxDma::reload_pending/try_with_reload_settled -> RadioRegisters::mac_rx_reload_pending -> descriptor_reload_pending`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production reload polling reaches the exact PAC read`

### E36 — RX descriptor reload request

- `id: E36`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_rx_set_dscr_reload`; `rust_target: mac_rx_dma::request_descriptor_reload`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> RxRing -> RxDma::request_reload -> RadioRegisters::request_mac_rx_descriptor_reload -> request_descriptor_reload`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production ring publication reaches the exact PAC write`

### E37 — RX append/recycle composition

- `id: E37`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::wDev_AppendRxBlocks`; `rust_target: RxStagePool::stage_dma_unit_recycle_bounded`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: Esp32s31RxDmaService`; `source_production_call_path: Esp32s31RxDmaService -> staged RX reload service -> RxStagePool::stage_dma_unit_recycle_bounded`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: split pure ownership transition from async orchestration and use generic comparison`
- `current_consumer: Workbench feature wifi-rx-runtime-primitive`; `allowed_consumer: none until rewrite`; `legacy_status: provider-controlled refinement verdict`
- `reason: the probe owns cold-ring setup, descriptor completion, polling, delays and semantic result encoding`

### E38 — STA TSF wakeup

- `id: E38`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_set_sta_tsf_wakeup`; `rust_target: RadioRegisters::set_station_tsf_wakeup`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: none`; `allowed_consumer: analysis/regression-only`; `legacy_status: executable leaf not connected to runtime`
- `reason: only the validation probe currently calls this production-like PAC method`

### E39 — TX CCA force

- `id: E39`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_tx_set_cca`; `rust_target: mac_tx_queue::set_cca_force`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::begin_mac_tx_timeout_abort/with_detached_mac_tx -> set_cca_force`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capabilities rx-tx-dma and timeout-error-recovery after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production timeout handling reaches the exact PAC leaf`

### E40 — TX trigger-flow state

- `id: E40`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_get_txq_in_trig_flow_state`; `rust_target: mac_tx_queue::trigger_flow_state`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::take_mac_tx_completion -> trigger_flow_state`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: production completion handling reaches the exact PAC read`

### E41 — TX queue enabled query

- `id: E41`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_is_txq_enabled`; `rust_target: mac_tx_queue::queue_enabled`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::with_detached_mac_tx -> queue_enabled`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma`; `legacy_status: untrusted candidate`
- `reason: production detach confirmation reaches the exact PAC read`

### E42 — TX queue valid query

- `id: E42`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_is_txq_valid`; `rust_target: mac_tx_queue::queue_valid`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::with_detached_mac_tx -> queue_valid`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma`; `legacy_status: untrusted candidate`
- `reason: production timeout and detach paths reach the exact PAC read`

### E43 — TX queue invalidation

- `id: E43`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_set_txq_invalid`; `rust_target: mac_tx_queue::invalidate_queue`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::with_detached_mac_tx(timeout) -> invalidate_queue`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capabilities rx-tx-dma and timeout-error-recovery`; `legacy_status: untrusted candidate`
- `reason: production timeout detach reaches the exact PAC write`

### E44 — TX queue disable

- `id: E44`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_txq_disable`; `rust_target: mac_tx_queue::disable_queue`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot/TxHardware -> RadioRegisters::with_detached_mac_tx -> disable_queue`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capabilities rx-tx-dma and timeout-error-recovery`; `legacy_status: untrusted candidate`
- `reason: production collision, timeout and completion detach paths reach the exact PAC write`

### E45 — TX queue publication

- `id: E45`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_txq_enable`; `rust_target: TxSlot::submit_legacy`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> Esp32s31ConnectedTx -> TxSlot::submit_legacy`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: move setup and extraction outside target and projection into claim`
- `current_consumer: Workbench feature wifi-tx-runtime-primitive`; `allowed_consumer: none until rewrite`; `legacy_status: provider-controlled refinement verdict`
- `reason: the probe constructs storage, reserves a slot, derives an image and checks state around the production call`

### E46 — TX EDCA configuration

- `id: E46`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_tx_config_edca`; `rust_target: mac_tx_queue::configure_edca`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner TX path`; `source_production_call_path: ConnectedRunner TX path -> TxSlot::submit_legacy -> TxHardware::prepare_bound_legacy_tx -> RadioRegisters::prepare_legacy_mac_tx -> configure_edca`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma`; `legacy_status: untrusted candidate`
- `reason: pointer-rich ABI conversion can stay outside the exact production register call`

### E47 — TX BlockAck payload

- `id: E47`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_tx_get_blockack`; `rust_target: RadioRegisters::read_tx_block_ack_payload`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedRunner aggregate TX path`; `source_production_call_path: ConnectedRunner aggregate TX path -> TxHardware::take_ht_ampdu_completion -> RadioRegisters::take_mac_ht_ampdu_completion -> read_tx_block_ack_registers -> read_tx_block_ack_payload`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: qualification capability rx-tx-dma`; `legacy_status: untrusted candidate`
- `reason: output projection can occur after the direct production PAC read`

### E48 — PHY AGC disable

- `id: E48`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: rom::phy_disable_agc`; `rust_target: hal::phy_agc::set_enabled(false)`
- `shipping_profile: esp32s31/wifi/phy-cold-start`; `shipping_root: run_phy_register`; `source_production_call_path: run_phy_register -> PhyRegisterTransition/PhyTargetPort -> radio_hal::set_agc_enabled -> hal::phy_agc::set_enabled(false)`
- `oracle_kind: lifted`; `terminal_match_capable: false`; `required_for_terminal_match: concrete-replay`
- `current_consumer: none`; `allowed_consumer: qualification capability rf-bb-initialization after replay and attestation`; `legacy_status: lifted agreement only`
- `reason: the production PHY transition reaches the exact HAL leaf`

### E49 — PHY IQ estimator enable

- `id: E49`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: negative_fixture`; `trust: false`
- `vendor_target: rom::phy_iq_est_enable`; `rust_target: PhyDcIqEstimateTransition (declared but not called)`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: not applicable to this binding`
- `current_consumer: legacy async-deadlines vendor anchor/provider path`; `allowed_consumer: analysis/regression-only`; `legacy_status: provider self-certification`
- `reason: the probe independently implements configure, start, delay, poll and activity decisions`

### E50 — STA TSF snapshot

- `id: E50`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: rom::hal_get_sta_tsf`; `rust_target: mac_tsf::snapshot_station_tsf`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: ConnectedControlCore`; `source_production_call_path: ConnectedControlCore -> ConnectedControlHardware::station_tsf -> RadioRegisters::station_tsf -> snapshot_station_tsf`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: none`; `allowed_consumer: future connected-control qualification`; `legacy_status: untrusted candidate`
- `reason: null/output ABI adaptation surrounds a direct production PAC transaction`

### E51 — AP TSF reset/start

- `id: E51`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_tsf_reset`; `rust_target: ap_tsf::reset_and_start_access_point_tsf`
- `shipping_profile: esp32s31/wifi-ap/embassy`; `shipping_root: Esp32s31ApEngine`; `source_production_call_path: Esp32s31ApEngine -> reset_and_start_access_point_tsf -> ApTsfHardware -> WifiMacHal::reset_and_start_access_point_tsf -> RadioRegisters::reset_and_start_softap_tsf`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance and selector-zero declared domain`
- `current_consumer: Workbench feature wifi-ap-tsf-start`; `allowed_consumer: future AP qualification`; `legacy_status: untrusted candidate`
- `reason: selector filtering is probe setup; the selected path calls the real AP runtime operation`

### E52 — AP TSF stop

- `id: E52`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_disable_softap_tsf`; `rust_target: ap_tsf::stop_access_point_tsf`
- `shipping_profile: esp32s31/wifi-ap/embassy`; `shipping_root: Esp32s31ApEngine`; `source_production_call_path: Esp32s31ApEngine -> stop_access_point_tsf -> ApTsfHardware -> WifiMacHal::stop_access_point_tsf -> RadioRegisters::stop_softap_tsf`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: Workbench feature wifi-ap-tsf-stop`; `allowed_consumer: future AP qualification`; `legacy_status: untrusted candidate`
- `reason: direct production LMAC, HAL and PAC chain`

### E53 — RX beacon PTI set

- `id: E53`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_set_rx_beacon_pti`; `rust_target: RadioRegisters::set_rx_beacon_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature wifi-mac-coex-register-programming`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the LMAC wrapper has tests but no shipping runtime caller`

### E54 — RX beacon PTI clear

- `id: E54`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_clear_rx_beacon_pti`; `rust_target: RadioRegisters::clear_rx_beacon_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature wifi-mac-coex-register-programming`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the LMAC wrapper has tests but no shipping runtime caller`

### E55 — individual-TWT PTI set

- `id: E55`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_set_itwt_pti`; `rust_target: RadioRegisters::set_itwt_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature wifi-mac-coex-register-programming`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the LMAC wrapper has tests but no shipping runtime caller`

### E56 — individual-TWT PTI clear

- `id: E56`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_clr_itwt_pti`; `rust_target: RadioRegisters::clear_itwt_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature wifi-mac-coex-register-programming`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the LMAC wrapper has tests but no shipping runtime caller`

### E57 — TX PTI set

- `id: E57`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_QUARANTINE`; `purpose: future-production-like`; `trust: false`
- `vendor_target: libpp::hal_set_tx_pti`; `rust_target: RadioRegisters::set_tx_pti`
- `shipping_profile: none`; `shipping_root: none`; `source_production_call_path: none`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: shipping integration plus new-model attestation`
- `current_consumer: Workbench feature wifi-mac-coex-register-programming`; `allowed_consumer: analysis/regression-only`; `legacy_status: static Workbench completion input`
- `reason: the LMAC wrapper has tests but no shipping runtime caller`

### E58 — MAC interface address

- `id: E58`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_set_addr`; `rust_target: RadioRegisters::program_receive_interface_address`
- `shipping_profile: esp32s31/wifi/cold-start`; `shipping_root: start_esp32s31_wifi_mac`; `source_production_call_path: start_esp32s31_wifi_mac -> initialize_wifi_mac -> program_cold_receive_addresses -> MacInterfaceAddressHardware::program_interface_address -> ColdRadioRegisters::program_receive_interface_address`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and full declared domain`
- `current_consumer: Workbench feature wifi-ap-sta-interface-identity`; `allowed_consumer: future STA/AP qualification`; `legacy_status: untrusted candidate`
- `reason: production cold-start publishes both typed interface addresses through the same PAC leaf`

### E59 — MAC interface BSSID

- `id: E59`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_ATTEST`; `purpose: production-claim`; `trust: false`
- `vendor_target: libpp::hal_mac_set_bssid`; `rust_target: RadioRegisters::program_interface_bssid`
- `shipping_profile: esp32s31/wifi-sta-and-ap`; `shipping_root: STA/AP role activation`; `source_production_call_path: STA configure_sta_link_receive_policy or AP configure_ap_receive_policy -> WifiMacHal -> RadioRegisters::apply_role_receive_policy -> program_interface_bssid`
- `oracle_kind: concrete-replay`; `terminal_match_capable: true`; `required_for_terminal_match: Stage-2 compiled provenance, target window and role-declared domain`
- `current_consumer: Workbench feature wifi-ap-sta-interface-identity`; `allowed_consumer: future STA/AP qualification`; `legacy_status: untrusted candidate`
- `reason: both shipping role-policy paths reach the typed production BSSID leaf`

### E60 — Role-specific key insertion

- `id: E60`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: wifi-key-role::wDev_Insert_KeyEntry`; `rust_target: install_sta_pairwise_ccmp/install_ap_pairwise_ccmp`
- `shipping_profile: esp32s31/wifi-sta-and-ap`; `shipping_root: WPA2 STA key port / AP security engine`; `source_production_call_path: STA WPA2 key port -> install_sta_pairwise_ccmp; AP security engine -> install_ap_pairwise_ccmp`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: split STA and AP claims, move fixtures to setup and use claim-owned projection`
- `current_consumer: Workbench feature wifi-ap-sta-key-role`; `allowed_consumer: none until rewrite`; `legacy_status: provider-controlled semantic-field comparison`
- `reason: the probe selects the implementation and invents peer, key and AID inputs before provider comparison`

### E61 — STA/AP receive policy dispatcher

- `id: E61`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: wifi-sta-ap-receive::wifi_set_rx_policy`; `rust_target: WifiMacHal::configure_role_receive_policy`
- `shipping_profile: esp32s31/wifi-sta-and-ap`; `shipping_root: STA/AP role activation`; `source_production_call_path: STA configure_sta_link_receive_policy -> WifiMacHal::configure_station_receive_policy; AP configure_ap_receive_policy -> WifiMacHal::configure_access_point_receive_policy`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: split role claims and remove probe-owned selector dispatch`
- `current_consumer: Workbench feature wifi-ap-sta-receive-registers`; `allowed_consumer: none until rewrite`; `legacy_status: probe-selected reviewed-domain comparison`
- `reason: the probe reproduces vendor case selection and mode mapping instead of entering the real role-specific shipping roots`

### E62 — Connected-STA beacon-filter disable

- `id: E62`; `proof_kind: effect-rust-probe`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: wifi-sta-lifecycle::hal_disable_sta_beacon_filter`; `rust_target: MacInterruptSetup::prepare_connected_sta_without_power_save`
- `shipping_profile: esp32s31/wifi-sta/embassy`; `shipping_root: connected STA entry`; `source_production_call_path: connected STA entry -> MacInterruptSetup::prepare_connected_sta_without_power_save -> disable_sta_beacon_filter -> device_fence`
- `oracle_kind: concrete-replay`; `terminal_match_capable: false`; `required_for_terminal_match: probe must call the declared production lifecycle API and preserve its fence/token path`
- `current_consumer: Workbench feature wifi-sta-connected-no-power-save`; `allowed_consumer: none until rewrite`; `legacy_status: helper-level exact-effects match`
- `reason: the validation probe bypasses the declared production method, its fence and its preparation token by calling the shared private helper directly`

## PHY semantic paths omitted by the former 62-binding audit

All eight paths below let the provider execute a Rust transition, normalize
both sides and compute the verdict. They are useful claims, but their provider
currently combines oracle support, executor, projection and comparator.

### S01 — PHY register initialization

- `id: S01`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::register_chipv7_phy`; `rust_target: PhyRegisterTransition`
- `shipping_profile: esp32s31/wifi/phy-cold-start`; `shipping_root: start_esp32s31_wifi`; `source_production_call_path: start_esp32s31_wifi -> run_phy_register -> PhyRegisterTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-register-init and ledger capability cold-registration (mapped)`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider drives production transition completions and compares its own normalized event vocabulary`

### S02 — PHY RF initialization

- `id: S02`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_rf_init`; `rust_target: PhyRfColdInit`
- `shipping_profile: esp32s31/wifi/phy-cold-start`; `shipping_root: start_esp32s31_wifi`; `source_production_call_path: start_esp32s31_wifi -> run_phy_register -> PhyRegisterTransition -> PhyRfColdInit`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-rf-init and ledger capability rf-bb-initialization (mapped)`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider drives transition completions, normalizes events and computes MATCH`

### S03 — PHY baseband initialization

- `id: S03`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_bb_init`; `rust_target: PhyBbInitTransition`
- `shipping_profile: esp32s31/wifi/phy-cold-start`; `shipping_root: start_esp32s31_wifi`; `source_production_call_path: start_esp32s31_wifi -> run_phy_register -> PhyRegisterTransition -> PhyBbInitTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-baseband-init and ledger capability rf-bb-initialization (mapped)`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider drives transition completions, normalizes events and computes MATCH`

### S04 — Bluetooth TX gain initialization

- `id: S04`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_bt_tx_gain_init`; `rust_target: PhyBluetoothTxGainInitTransition`
- `shipping_profile: esp32s31/shared-PHY`; `shipping_root: PhyBbInitTransition`; `source_production_call_path: PhyBbInitTransition -> PhyBluetoothTxGainInitTransition -> subordinate Bluetooth transitions`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-bluetooth-tx-gain-init`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider supplies completion behavior, creates the event trace and computes MATCH`

### S05 — PHY channel selection

- `id: S05`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_chip_set_chan`; `rust_target: PhyChipChannelTransition`
- `shipping_profile: esp32s31/wifi-sta/production`; `shipping_root: Esp32s31ScanPhy::switch_channel`; `source_production_call_path: Esp32s31ScanPhy::switch_channel -> switch_phy_channel_with_mac_restart -> PhyChipChannelTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: qualification capability channel-selection-switch`; `allowed_consumer: none until rewrite`; `legacy_status: vendor-proof qualified under old provider-computed semantics; trusted_status false`
- `reason: this is the only terminal legacy ledger result affected; provider executes the Rust transition, normalizes both traces and computes MATCH`

### S06 — Bluetooth TX DC calibration

- `id: S06`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_bt_txdc_cal_new`; `rust_target: PhyBluetoothTxDcTransition`
- `shipping_profile: esp32s31/shared-PHY`; `shipping_root: PhyBluetoothTxGainInitTransition`; `source_production_call_path: PhyBluetoothTxGainInitTransition -> PhyBluetoothTxDcTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-bluetooth-txdc`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider drives calibration responses, constructs normalized events and computes MATCH`

### S07 — Bluetooth TX power calibration

- `id: S07`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_bt_tx_pwctrl_init`; `rust_target: PhyBluetoothTxPowerTransition`
- `shipping_profile: esp32s31/shared-PHY`; `shipping_root: PhyBluetoothTxGainInitTransition`; `source_production_call_path: PhyBluetoothTxGainInitTransition -> PhyBluetoothTxPowerTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-bluetooth-tx-power`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider supplies hardware completions, constructs normalized events and computes MATCH`

### S08 — Bluetooth TX DC power-detector calibration

- `id: S08`; `proof_kind: phy-semantic`; `classification: ACCEPT_REWRITE`; `purpose: production-claim`; `trust: false`
- `vendor_target: archive::phy_txdc_cal_pwdet_init`; `rust_target: PhyBluetoothTxDcPwdetTransition`
- `shipping_profile: esp32s31/shared-PHY`; `shipping_root: PhyBluetoothTxGainInitTransition`; `source_production_call_path: PhyBluetoothTxGainInitTransition -> PhyBluetoothTxDcPwdetTransition`
- `oracle_kind: concrete-replay-plus-provider-model`; `terminal_match_capable: false`; `required_for_terminal_match: generic execution/projection/verdict with compiled production provenance`
- `current_consumer: legacy semantic contract esp32s31-bluetooth-txdc-pwdet`; `allowed_consumer: none until rewrite`; `legacy_status: provider-computed semantic result`
- `reason: provider drives calibration responses, constructs normalized events and computes MATCH`

## Stage 1 result

```text
Trust surface
=============

Effect/rust-probe paths       62
PHY semantic paths             8
Total                         70

ACCEPT_ATTEST                 31
ACCEPT_REWRITE                16
ACCEPT_QUARANTINE             23
REMOVE                         0
NEEDS_REVIEW                   0

Trusted under new model        0

Quarantine:
  future-production-like      21
  negative fixtures            2

Rewrite:
  effect/probe                 8
  PHY semantic                 8

Lifted-oracle, non-terminal   15

Legacy terminal qualification affected:
  channel-selection-switch
    <- phy_chip_set_chan
```

The fifteen lifted, non-terminal paths are E14, E15, E27 through E36, E39,
E40 and E48. They may expose `DIFF`, `INCOMPLETE` or analysis agreement, but
cannot produce terminal production vendor-proof until concrete replay exists.

Stage 1 authorizes no implementation or trust migration. Stage 2 must begin
with three small claims and must reject a deliberately copied probe
implementation even if its effects match. It must also demonstrate that a
shipping-code mutation changes `MATCH` to `DIFF`, while mutating a quarantined
shadow model cannot change qualification.
