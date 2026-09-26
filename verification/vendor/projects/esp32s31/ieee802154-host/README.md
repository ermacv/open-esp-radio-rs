# ESP32-S31 IEEE 802.15.4 host stand

This workspace runs the public ESP-IDF IEEE 802.15.4 driver on the host so the
production port can be compared with the real vendor behavior. Unlike the PHY,
Wi-Fi and Bluetooth libraries, this driver is published as source, so the stand
compiles it directly instead of executing a captured binary. The
[driver port map](../reference/ieee802154-driver-port.md) identifies the pinned
sources and their production owners.

## What is compiled and what is recorded

[`vendor-sources.toml`](vendor-sources.toml) lists every translation unit and
every ESP-IDF header the build uses, with its SHA-256 at the pinned revision.
[`build.rs`](build.rs) refuses to build unless each file matches. The driver
sources are compiled unmodified with the Kconfig defaults in
[`shim/include/sdkconfig.h`](shim/include/sdkconfig.h).

The stand replaces only the boundaries the driver does not own:

| Boundary | Replacement |
| --- | --- |
| Every `ieee802154_ll_*` register accessor | Generated from the real `ieee802154_common_ll.h`: the vendor types and constants stay, each accessor records its name and arguments |
| Direct `REG_READ`/`REG_WRITE` (the ETM helpers) | Recorded register reads and writes |
| PHY, BTBB, coexistence, modem clocks, `ieee802154_txon_delay_set`, `bt_bb_get_cur_rx_info` | Recorded calls ([`shim/src/host.c`](shim/src/host.c)) |
| `esp_intr_alloc` | Records the allocation and captures the handler that scenarios invoke |
| `esp_timer_get_time`, the driver spinlock, `assert` | Recorded; a failed assertion becomes a trace record |
| Application callbacks (`esp_ieee802154_receive_done`, ...) | Recorded with their frame bytes and frame information |

Register-layer getters answer from scenario inputs, or with the last value
the matching setter wrote. The model supplies nothing else: event images,
abort reasons and received frames are explicit scenario steps.

## Obtaining the sources

Fetch the pinned revision into an ignored directory and point
`OER_ESP_IDF_DIR` at it:

```console
git init target/esp-idf && cd target/esp-idf
git remote add origin https://github.com/espressif/esp-idf.git
git sparse-checkout set components/ieee802154 components/esp_hal_ieee802154 \
    components/esp_coex/include components/soc/esp32s31/include \
    components/soc/esp32s31/register
git fetch --depth 1 --filter=blob:none origin 7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe
git checkout FETCH_HEAD
```

The stand needs a host C compiler.

## Running

```console
export OER_ESP_IDF_DIR=$PWD/target/esp-idf
cargo test --manifest-path verification/vendor/projects/esp32s31/ieee802154-host/Cargo.toml
cargo run --manifest-path verification/vendor/projects/esp32s31/ieee802154-host/Cargo.toml -- list
cargo run --manifest-path verification/vendor/projects/esp32s31/ieee802154-host/Cargo.toml -- run transmit-with-ack
cargo run --manifest-path verification/vendor/projects/esp32s31/ieee802154-host/Cargo.toml -- compare transmit-with-ack
```

The compiled driver keeps its state in C globals, so each process runs one
scenario. `run` prints the scenario's boundary trace, one record per line;
buffer addresses appear as `tx#n` (the `n`th transmitted frame) and `buf#n`
(the `n`th distinct driver buffer). The [tests](tests/catalog.rs) check the
catalog against expectations read from the pinned source.

## Comparing the production engine

`compare` runs the same scenario through the production engine
(`oer_esp32s31_ieee802154::engine`) and prints `MATCH`, `DIFF` with the first
differing record, or `INCOMPLETE` with the reason. [`src/port.rs`](src/port.rs)
renders each HAL low-level call as the vendor `ieee802154_ll_*` accessor and
argument, and each modem ETM access as the vendor's direct register access;
getters on both sides answer from one shared value model. Calls that leave
the driver (clocks, PHY, BTBB, interrupt allocation, time, critical sections)
are not compared. A vendor assertion, or a step the engine does not own, is
`INCOMPLETE`. Address, extended-address and key arguments are compared by
content. The stand has no BTBB power table, so both sides resolve power
to index zero. The tests require `MATCH` for every catalog scenario.

## Limits

The stand exercises the driver's software behavior against scripted register
values. It makes no claim about register effects, hardware timing, concurrent
interrupts or RF behavior: register effects of the LL accessors are compared
separately with the PAC, and hardware behavior belongs to HIL.
