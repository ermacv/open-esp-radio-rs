# Реестр крейтов driver

Снимок: `134a75ac6f0eeeb60a76fca22d0bfbf51b1f4013`, 2026-09-04. Все 44 манифеста. Пути назначения описывают первый перенос контейнеров; внутреннее разбиение и возможное объединение крейтов указаны в основном отчёте. Число строк включает тесты, комментарии и пустые строки. Вложенные крейты не входят в размер родительского крейта.

| Текущий корень | Cargo package | Workspace | Rust файлов / строк | Предлагаемый корень |
|---|---|---|---:|---|
| `driver/adapters/embassy/esp32s31-bluetooth` | `open-esp-radio-esp32s31-bluetooth-embassy` | root | 17 / 12567 | `driver/adapters/embassy/esp32s31/bluetooth` |
| `driver/adapters/embassy/esp32s31-coex` | `open-esp-radio-esp32s31-coex-embassy` | root | 1 / 263 | `driver/adapters/embassy/esp32s31/coex` |
| `driver/adapters/embassy/esp32s31-ieee802154` | `open-esp-radio-esp32s31-ieee802154-embassy` | root | 2 / 907 | `driver/adapters/embassy/esp32s31/ieee802154` |
| `driver/adapters/embassy/esp32s31-platform` | `open-esp-radio-esp32s31-embassy-runtime` | root | 3 / 402 | `driver/adapters/embassy/esp32s31/runtime` |
| `driver/adapters/embassy/esp32s31-wifi` | `open-esp-radio-esp32s31-wifi-embassy` | root | 157 / 68946 | `driver/adapters/embassy/esp32s31/wifi` |
| `driver/adapters/embassy/esp32s31-wifi-compat` | `open-esp-radio-esp32s31-wifi-embassy-compat` | root | 3 / 929 | `driver/adapters/embassy/esp32s31/wifi-compat` |
| `driver/adapters/embassy/wifi` | `open-esp-radio-wifi-embassy` | root | 5 / 1902 | `driver/adapters/embassy/wifi` |
| `driver/adapters/embassy-net` | `open-esp-radio-embassy-net` | root | 2 / 799 | `driver/adapters/network/embassy/owned` |
| `driver/adapters/embassy-net-compat` | `open-esp-radio-embassy-net-compat` | root | 2 / 1099 | `driver/adapters/network/embassy/compat` |
| `driver/adapters/esp-hal/esp32s31-ieee802154` | `open-esp-radio-esp32s31-ieee802154-esp-hal` | root | 1 / 182 | `driver/adapters/esp-hal/esp32s31/ieee802154` |
| `driver/adapters/esp-hal/esp32s31-radio-platform` | `open-esp-radio-esp32s31-radio-platform-esp-hal` | root | 6 / 1361 | `driver/adapters/esp-hal/esp32s31/radio` |
| `driver/adapters/esp-hal/esp32s31-wifi` | `open-esp-radio-esp32s31-wifi-esp-hal` | root | 2 / 402 | `driver/adapters/esp-hal/esp32s31/wifi` |
| `driver/adapters/research` | `open-esp-radio-research-datapath` | root | 6 / 1724 | `driver/adapters/network/research` |
| `driver/bluetooth/hci` | `open-esp-radio-bluetooth-hci` | root | 10 / 11352 | `driver/bluetooth/hci` |
| `driver/bluetooth/ll` | `open-esp-radio-bluetooth-ll` | root | 8 / 4201 | `driver/bluetooth/ll` |
| `driver/chips/esp32s31/bluetooth` | `open-esp-radio-esp32s31-bluetooth` | root | 91 / 63851 | `driver/chips/esp32s31/bluetooth` |
| `driver/chips/esp32s31/bluetooth/memory` | `open-esp-radio-esp32s31-bluetooth-memory` | root | 23 / 15954 | `driver/chips/esp32s31/bluetooth/memory` |
| `driver/chips/esp32s31/coex` | `open-esp-radio-esp32s31-coex` | root | 7 / 905 | `driver/chips/esp32s31/coex` |
| `driver/chips/esp32s31/hal` | `open-esp-radio-esp32s31-hal` | root | 33 / 16625 | `driver/chips/esp32s31/hal` |
| `driver/chips/esp32s31/ieee802154/dma` | `open-esp-radio-esp32s31-ieee802154-dma` | root | 7 / 1997 | `driver/chips/esp32s31/ieee802154/dma` |
| `driver/chips/esp32s31/ieee802154/irq` | `open-esp-radio-esp32s31-ieee802154-irq` | root | 2 / 818 | `driver/chips/esp32s31/ieee802154/irq` |
| `driver/chips/esp32s31/ieee802154/mac` | `open-esp-radio-esp32s31-ieee802154-mac` | root | 3 / 2169 | `driver/chips/esp32s31/ieee802154/mac` |
| `driver/chips/esp32s31/ieee802154/runtime` | `open-esp-radio-esp32s31-ieee802154-runtime` | root | 2 / 2562 | `driver/chips/esp32s31/ieee802154/runtime` |
| `driver/chips/esp32s31/pac` | `open-esp-radio-esp32s31-pac` | root | 67 / 35685 | `driver/chips/esp32s31/pac` |
| `driver/chips/esp32s31/pac-raw` | `open-esp-radio-esp32s31-pac-raw` | root | 4 / 73042 | `driver/chips/esp32s31/pac/raw` |
| `driver/chips/esp32s31/phy` | `open-esp-radio-esp32s31-phy` | root | 44 / 58034 | `driver/chips/esp32s31/phy` |
| `driver/chips/esp32s31/platform-pac` | `open-esp-radio-esp32s31-platform-pac` | root | 5 / 1419 | `driver/adapters/esp-hal/esp32s31/soc` |
| `driver/chips/esp32s31/wifi` | `open-esp-radio-esp32s31-wifi` | root | 14 / 4674 | `driver/chips/esp32s31/wifi` |
| `driver/chips/esp32s31/wifi/ap` | `open-esp-radio-esp32s31-wifi-ap` | root | 8 / 8511 | `driver/chips/esp32s31/wifi/ap` |
| `driver/chips/esp32s31/wifi/dma` | `open-esp-radio-esp32s31-wifi-dma` | root | 7 / 8316 | `driver/chips/esp32s31/wifi/dma` |
| `driver/chips/esp32s31/wifi/mac` | `open-esp-radio-esp32s31-wifi-mac` | root | 52 / 25611 | `driver/chips/esp32s31/wifi/mac` |
| `driver/chips/esp32s31/wifi/sta` | `open-esp-radio-esp32s31-wifi-sta` | root | 19 / 14108 | `driver/chips/esp32s31/wifi/sta` |
| `driver/common/dma` | `open-esp-radio-dma` | root | 5 / 2298 | `driver/common/dma` |
| `driver/common/network` | `open-esp-radio-network` | root | 1 / 58 | `driver/common/network` |
| `driver/ieee802154` | `open-esp-radio-ieee802154` | root | 6 / 1488 | `driver/ieee802154` |
| `driver/integration/esp32s31/bluetooth` | `open-esp-radio-esp32s31-bluetooth-integration` | root | 8 / 3636 | `driver/integration/esp32s31/embassy/bluetooth` |
| `driver/integration/esp32s31/embassy-wifi` | `open-esp-radio-esp32s31-embassy-wifi` | isolated | 19 / 10586 | `driver/integration/esp32s31/embassy/wifi` |
| `driver/radio` | `open-esp-radio` | root | 5 / 3554 | `driver/radio` |
| `driver/wifi/ap` | `open-esp-radio-wifi-ap` | root | 2 / 3396 | `driver/wifi/ap` |
| `driver/wifi/datapath` | `open-esp-radio-wifi-datapath` | root | 2 / 844 | `driver/wifi/datapath` |
| `driver/wifi/ieee80211` | `open-esp-radio-ieee80211` | root | 28 / 18350 | `driver/wifi/ieee80211` |
| `driver/wifi/softmac` | `open-esp-radio-wifi-softmac` | root | 6 / 4891 | `driver/wifi/softmac` |
| `driver/wifi/sta` | `open-esp-radio-wifi-sta` | root | 9 / 6440 | `driver/wifi/sta` |
| `driver/wifi/wpa2` | `open-esp-radio-wpa2` | root | 9 / 6221 | `driver/wifi/wpa2` |

Полный список объявленных зависимостей, включая optional, target-specific и dev: [dependencies.csv](dependencies.csv). Это объединение деклараций, а не граф одной конфигурации. Матрица реально разрешённых конфигураций приведена в основном отчёте.
