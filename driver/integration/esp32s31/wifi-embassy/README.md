# ESP32-S31 Wi-Fi / Embassy integration

This crate owns the reusable async composition between ESP32-S31 Wi-Fi
hardware owners and a bounded Embassy network adapter. It does not own board
startup, task placement, credentials, sockets, traffic generation or HIL
reporting.

The connected RX path exposes an optional typed `RxPipelineObserver`. This
crate defines when semantic pipeline events occur, but does not choose
counters, sampling, report formats or IRQ correlation. Those concrete
qualification policies belong to the consuming HIL.

Associated-peer policy is not an Embassy concern. Candidate preparation and
WMM/HT/HE/rate-control programming live in
`open_esp_radio_esp32s31_wifi_sta::peer`; this crate contains only the private binding
from its narrow transmit trait to the async control-TX owner.

Likewise, STA TX entropy, calibrated-power and timer contracts plus their
resource bundle live in `open_esp_radio_esp32s31_wifi_sta::tx`. This crate
supplies only `tx_time::EmbassyWifiTxTimer`; it does not make Embassy time part
of station policy or Association power derivation.

The station-wide TX epoch itself lives in
`open_esp_radio_esp32s31_wifi_sta::tx_epoch`. The local `sta_tx_epoch` module
is only an extension that creates and restores this owner using the concrete
async control transmitter.

The persistent PHY/channel owner is
`open_esp_radio_esp32s31_wifi_sta::channel::Esp32s31ScanPhy`. Local scan and
attempt modules only adapt it to their async transaction traits; they do not
own its state or channel-switch policy.

The finite scan transaction, its failure taxonomy and primitive port are in
`open_esp_radio_esp32s31_wifi_sta::scan`. This crate retains the concrete
PHY/RX-DMA/probe-TX composition and maps one neutral dwell tick to Embassy
time.

The concrete scan adapter has four non-overlapping modules:

- `scan_rx` owns prepared/live/halted DMA-ring transitions and frame copying;
- `scan_tx` owns active Probe Request publication and passive fallback;
- `scan_port` composes PHY, RX, TX, storage and the executor dwell timer;
- `scan_target` implements those ports for cold and cooperative RISC-V owners.

There is no separate `running_scan` facade: cold scan and reconnect scan use
the same `Esp32s31ScanPort` with different typed resource owners.

RX descriptor and DMA-buffer storage is executor-independent and lives in
`open_esp_radio_esp32s31_wifi_lmac::rx_storage`. The local `rx_backend`
chooses the production large-RX dimensions and combines that arena with
reload waits, staging leases and optional observations. Network publication
is isolated in `network_rx`; the bounded borrowed-RX to owned-control handoff
is isolated in `control_mailbox`.

WPA2 protocol deadlines and atomic key-publication rollback live in
`open_esp_radio_wpa2::runner`, while the executor-independent ESP32-S31
handshake/key ports live in `open_esp_radio_esp32s31_wifi_sta::wpa2`. The local
private `wpa2_time` and public `wpa2_port` modules now provide only Embassy
time, retained DMA RX and control-TX bindings.

Open Authentication/Association deadlines and retry sequencing live in
`open_esp_radio_wifi_sta::join`. Its ESP32-S31 RX, control-TX, observer and
error contracts live in `open_esp_radio_esp32s31_wifi_sta::join`; the local
`sta_join_port` module only binds them to retained DMA RX and the concrete
control transmitter. Likewise the complete finite S31 attempt transaction and
its value-only input/report types live in
`open_esp_radio_esp32s31_wifi_sta::attempt`; private `join_time` plus the
concrete target module retain only Embassy time and the DMA/TX owner graph.

The application-facing station entry points are in `station`:

- `Esp32s31Station` owns the finite scan/join/connected/reconnect lifecycle;
- `Esp32s31StationController` publishes ordered reconnect/disconnect/stop
  requests;
- `run_esp32s31_connected_station_epoch` stops the radio only at a safe
  transaction boundary;
- `stop_esp32s31_connected_task_group` returns all spawned-task ownership or a
  distinct reset-required outcome under one deadline.

Run the host-side composition example with:

```text
cargo run -p open-esp-radio-esp32s31-wifi-embassy --example station_service
```

The example deliberately supplies a small deterministic lifecycle backend. A
real RISC-V board application binds the same facade to
`Esp32s31StaAttemptTargetPort`, its static DMA/network storage, its executor
spawners and the ESP-HAL interrupt adapter. Those board resources cannot be
made portable by hiding them in the driver or in global HIL state.
