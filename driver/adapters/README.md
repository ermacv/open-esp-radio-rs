# Reusable adapters

This directory contains adapters that compose core radio crates with external
Rust ecosystems. They are production-reusable boundaries, not board support
or test harnesses.

```text
network/embassy-net
    executor-neutral frame ownership for embassy-net-driver

esp32s31/wifi-embassy
    ESP32-S31 Wi-Fi DMA, IRQ and Embassy runtime composition

esp32s31/wifi-esp-hal
    esp-hal peripheral-singleton binding for the ESP32-S31 Wi-Fi backend
```

The generic network adapter intentionally depends on `embassy-net-driver` and
`embassy-sync`, not the complete `embassy-net` stack. The concrete
`embassy-net`/smoltcp application, board clocks, bootstrap, PSRAM/flash layout,
executor tasks and test scenarios belong to `../../hil/targets/esp32s31`.

Core PAC, HAL, PHY and protocol crates must not depend on this directory.
