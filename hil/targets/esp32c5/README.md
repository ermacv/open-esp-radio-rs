# ESP32-C5 HIL target

This workspace builds the esp32c5's HIL runtime, `oer-hil-esp32c5-runtime`,
for `riscv32imac-unknown-none-elf`. The chip boots through the ESP-IDF
second-stage bootloader of [`hil/bootloaders/esp32c5`](../../bootloaders/esp32c5/),
which loads an esp-hal application linked with `linkall.x`
([chip profile](../../../platform/esp32c5/chip.toml)). The runtime runs
Embassy on the esp-rtos scheduler and time driver.

The only image so far is `boot-smoke`: it reports `OPEN_RADIO_HIL hal=INIT`
after esp-hal initialization, `OPEN_RADIO_HIL embassy=START` once the
scheduler runs, and `OPEN_RADIO_HIL boot-smoke=PASS timer=PASS` after one
timer wake, on the USB Serial/JTAG console. It does not speak the HIL wire
protocol, and the runner does not build or run esp32c5 images yet; build and
flash it by hand:

```console
cd hil/targets/esp32c5
cargo build --release --features boot-smoke
cd ../../..
cargo hil flash --board esp32c5 --image hil-esp32c5-boot-smoke --monitor 20s \
    --until boot-smoke=PASS \
    hil/targets/esp32c5/target/riscv32imac-unknown-none-elf/release/oer-hil-esp32c5-runtime
```

Flashing replaces the board's peer firmware; a run that needs the peer
reflashes its catalog image first.
