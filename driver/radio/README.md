# `open-esp-radio`

Application facade for configuring and composing the source-only radio stack.
The configuration API has two phases:

1. an application describes the protocol subsystems and Wi-Fi owner topology;
2. a concrete chip/runtime composition validates capabilities before moving
   peripheral, DMA or task ownership.

For the currently implemented ESP32-S31 station topology:

```rust
use open_esp_radio::{
    RadioConfig, WifiConfig, WifiMacAddress, WifiStationConfig, esp32s31,
};

let station_address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
let requested = RadioConfig::wifi(WifiConfig::station(WifiStationConfig::new(
    station_address,
)));
let plan = requested
    .validate(esp32s31::RADIO_CAPABILITIES)
    .unwrap();

assert!(plan.wifi().and_then(|wifi| wifi.station()).is_some());
```

A standalone capture request narrows to a role-specific plan before any S31
DMA owner is constructed:

```rust
use open_esp_radio::{RadioConfig, WifiConfig, WifiMonitorConfig, esp32s31};

let plan = RadioConfig::wifi(WifiConfig::monitor(WifiMonitorConfig::normalized()))
    .validate(esp32s31::RADIO_CAPABILITIES)
    .unwrap();
let monitor = plan.standalone_wifi_monitor().unwrap();
assert_eq!(monitor.monitor(), WifiMonitorConfig::normalized());
```

The plan contains no credentials, executor handles or memory buffers. A STA
network selection is a later request on the station service; AP beacon and
security policy similarly belong to the AP service. A passive monitor is a
best-effort RX tap and never consumes a VIF slot.

ESP32-S31 currently implements two exclusive Wi-Fi owner graphs: one station,
or one standalone normalized monitor. It does not yet keep promiscuous capture
active with an associated station. The monitor sink receives a synchronous
borrow and must copy into its own bounded storage; `Full` drops only that
observation and never delays RX descriptor recycling. The chip-neutral
Wi-Fi/Embassy adapter provides this storage as `MonitorCapturePool` and exposes
a non-blocking sink plus an async receiver. Frame arrays live only in the pool;
the channel retains small leases and metadata. The pool is not DMA backing,
but its current ownership bookkeeping uses atomics, so external-RAM placement
must be explicitly qualified for the target rather than assumed from CPU
readability.

With `esp32s31-wifi-embassy`, the complete public vocabulary is under
`open_esp_radio::esp32s31::wifi::embassy::monitor`: the S31 RX and interrupt
service plus the chip-neutral capture pool/sink/receiver. The service consumes
the checked standalone plan, starts the DMA walker, activates the qualified
interrupt epoch and permits owner extraction only after a cooperative stop.
Its borrowed run future and fail-closed destructor prevent cancellation or
scope exit from silently destroying an active DMA/ISR owner. Failure to
confirm shutdown enters the platform reset boundary.

On ESP32-S31 firmware, `start_esp32s31_radio` performs the same validation
before the unique unpowered `Radio` owner moves. A successful
`Esp32s31StartedRadio` retains both the checked `RadioPlan` and the powered,
calibrated Wi-Fi owner. Configuration failure returns the original `Radio`;
hardware failure returns the owner at the exact failed transition. The
selected station VIF is then passed unchanged into the connected owner graph,
so reconnects cannot silently revert to an implicit interface.

Bluetooth and IEEE 802.15.4 can already be selected as distinct subsystem
requests so that unsupported combinations fail before initialization. Their
protocol-specific configuration and handles will be introduced only with real
owner graphs rather than speculative placeholder APIs.
