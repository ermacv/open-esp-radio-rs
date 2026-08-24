# ESP32-S31 IEEE 802.15.4 lifecycle source review

This note records findings derived from public ESP-IDF source at commit
`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe` and separately labels bounded HIL
observations from real ESP32-S31 hardware. It is a research input, not a
production-equivalence decision. The overall lifecycle verdict remains
**INCOMPLETE**: clock/reset order, MAC foundation writes, and channel coding are
source-confirmed, but RF/BB initialization contains opaque calls and the
`EVENT_STATUS` access class is not stated by the reviewed register headers or
resolved by the HIL observation below.

No vendor-artifact-derived values are used here. Unless a section explicitly
identifies a dated HIL observation, numeric results below are either literal
source values or arithmetic expansion of public macros and enums.

## Source ledger

The files were downloaded from the pinned commit and hashed locally before
review. Links include the line ranges used for the findings.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154.c#L35-L48) | public enable/disable order | `a83716d9944d4ffba1998cc64ebb635a605b60fc77c74ae6070e83a1c617f1bc` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L897-L977) | module enable, MAC init/deinit | `9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L1193-L1207) | guarded PHY client acquire/release | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c#L17-L39) | TX-power table dependency | `4bc94779b0c29fdfc77dcdf0c6d3d66fad5d02324aa951d9f19877bc62532cf4` |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c#L56-L127) | PIB defaults and deferred hardware update | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c#L14-L60) | channel conversion and default coexistence scenes | `e1a012d5f359e2445128977e82a304ba94c100c2994729e062e26586596df38a` |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c#L88-L92) | public weak TX-power-table fallback | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_debug.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_debug.c#L318-L324) | software statistic clear | `e4dd37b1ffc462c78a12cca7d57e4e8bd4e0e8984d542012b12ce964ee9a1812` |
| [`components/ieee802154/private_include/esp_ieee802154_util.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_util.h#L21-L35) | valid channel range | `4ca86544b16248e1d66b85cc84df8ac37bde50727189b3b242513aab22e19017` |
| [`components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c#L7-L12) | S31 module and IRQ selection | `56246b6b482752e0d217e2391acb4869ae02cd18068c5ee3b361a4b7ae110995` |
| [`components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h#L7-L12) | S31 LL includes the common LL | `a66a3562ff6ef62ffa6dda90bc08e97f0d4509bcf3817cdc57795fd97daeaa30` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L46-L122) | event and abort values | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L168-L222) | ED mode, event RMW, abort-enable RMW | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L265-L277) | frequency-code and power writes | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L362-L364) | ED-average field write | same file/hash as above |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L476-L480) | non-coexistence PTI values | same file/hash as above |
| [`components/soc/esp32s31/register/soc/ieee802154_reg.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_reg.h#L184-L194) | event masks; no access-class annotation | `fd3f944ac97634605083031f96c0f942af26a81a9e9a3123281c59e5719f9d9c` |
| [`components/soc/esp32s31/register/soc/ieee802154_struct.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_struct.h#L143-L151) | event-status bitfield; no access-class annotation | `da13c2bc78cd6ef35a4e54ddddf11ce48fda967746193f1a0ad03578a5881752` |
| [`components/esp_hw_support/modem/modem_clock.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/modem_clock.c#L57-L149) | low-bit-first loop, refcounts, ICG-before-dependencies, reset dispatch | `f6cd061416a1356a7ca3cadb84ad9d8c137971cd9c167a74507ecd57d80f7edd` |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L15-L55) | S31 dependency sets | `c89ae6110cb4ca4e1208db3ef7263c729c84c84b2b5edaed3e9a3ce7c3c1e4df` |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L76-L225) | concrete dependency actions | same file/hash as above |
| [`components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c#L299-L400) | device dispatch/refcounts and ICG defaults | same file/hash as above |
| [`components/esp_hw_support/modem/include/modem/modem_clock_impl.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/include/modem/modem_clock_impl.h#L23-L83) | device bit numbering | `497fbee5c4e07adb01cfa5980ee58dbf9ecbffde6b60855f8d5df82fc9027bc7` |
| [`components/esp_hw_support/modem/include/modem/modem_clock_impl.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/modem/include/modem/modem_clock_impl.h#L104-L158) | refcount flag and ICG codes | same file/hash as above |
| [`components/esp_hw_support/include/esp_private/esp_modem_clock.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/include/esp_private/esp_modem_clock.h#L25-L69) | public shared-clock ownership contract | `3be56e98b6c110e655186e83a5c55e72879ef4ee35e668fc172aedd4334952ff` |
| [`components/hal/esp32s31/modem_clock_hal.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/modem_clock_hal.c#L23-L129) | ICG writes and PLL-source clock image | `53c1f8c25b86cbb8eff5d06633615dde605763310fa97bcd3261e9bd84cce56e` |
| [`components/hal/include/hal/modem_clock_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/include/hal/modem_clock_types.h#L15-L35) | ICG domain order | `e00978e9a623b411467d821256414e783095bedd226e6ef32b00904fcfe21075` |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L83-L145) | ETM, ZB APB/MAC, modem-security APB fields | `565e2246dfd4af00c073b8668ee0b81b2eef9992b24030d814055159fec30b0c` |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L337-L348) | ZBMAC reset pulses | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L475-L477) | Wi-Fi-BB 80x1 field | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_syscon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_syscon_ll.h#L598-L630) | BT APB/BB fields | same file/hash as above |
| [`components/hal/esp32s31/include/hal/modem_lpcon_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/hal/esp32s31/include/hal/modem_lpcon_ll.h#L173-L183) | coexistence clock field | `5e6ce588f2e029aa3b895b57b0cfdad25efeacfc3cea835cdc320bcd06d5608f` |
| [`components/esp_hal_pmu/include/hal/pmu_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_pmu/include/hal/pmu_types.h#L28-L37) | ICG sleep/modem/active bit positions | `a33d59247fb46ede4a53bcbf1402498d343f8cffda2216c0bdb55a39531df5ed` |
| [`components/soc/esp32s31/include/soc/soc_caps.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/soc_caps.h#L137-L140) | Wi-Fi/BT/802.15.4 feature gates | `002387dadb652c01af2fd606d90131a67eef24dbe74234a5ae6b0bb0d32ab1e4` |
| [`components/soc/esp32s31/include/soc/soc_caps.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/soc_caps.h#L512-L566) | independent modem clocks and ICG feature gates | same file/hash as above |
| [`components/esp_hw_support/periph_ctrl.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/periph_ctrl.c#L170-L244) | temporary PHY-calibration clock acquisition | `0489dd751eeb7698d8b4567a4a5b619cf8bb4a3c9e8cc84f517d5211d85d2561` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L332-L475) | PHY client-set lifecycle | `1e230e72f91c4b11f35b6b623dc45cf8628961593e45774e2bf610dce9896fbf` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L997-L1023) | opaque PHY registration/calibration call | same file/hash as above |
| [`components/esp_phy/src/phy_common.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_common.c#L36-L164) | PHY client-mask operations and PLL-tracking scheduler | `c602f4f6364f8fe726766bf0151e326194c82bdfe8c449b99a103225cdf89c80` |
| [`components/esp_phy/CMakeLists.txt`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/CMakeLists.txt#L15-L18) | unconditional `phy_common.c` inclusion when PHY is enabled | `ff79172451691f9cc589a323c78e35727ef2b3bf657e9ca998deb9ce15803ac1` |
| [`components/esp_phy/Kconfig`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/Kconfig#L181-L220) | PLL-tracking period and debug-only disable gate | `9e0bc9cafe445778c19e065e9b64be1689f81992dca4391d690a9fa633e4c632` |
| [`components/esp_phy/include/esp_phy_init.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_phy_init.h#L34-L42) | PHY client flag values | `700bf7b3865705b6d858e63a55fd5a880ebf298f700927cd642bee7536e4dc58` |
| [`components/esp_phy/include/esp_private/phy.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/phy.h#L38-L182) | PHY declarations, including opaque RF wake/close effects | `04027a4f11a0cd6c6a76478d681f7e29e4ba8ecf038d120d991bcab01735a53f` |
| [`components/esp_phy/src/btbb_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/btbb_init.c#L98-L139) | BTBB first-user initialization/refcount | `bde0cddaa033d2f34a4eaf0f1994b2d417850dba9d2fc5e6e9ee0ceb7caca3c3` |
| [`components/esp_phy/include/esp_private/btbb.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/btbb.h#L14-L20) | opaque BTBB declaration | `659a94ca15d9e7d5531f34c755476523d285bc2cbc0a378b58505e67facea289` |
| [`components/esp_coex/include/esp_coex_i154.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_coex/include/esp_coex_i154.h#L14-L41) | coexistence priorities and opaque setters | `4989f8d5a99300cf75419d099d735e38fb27be3f083bb79e7abe8a5af0d34f3e` |

## 1. Enable, clock, and reset order

The public enable path is strictly:

1. `ieee802154_enable()` acquires `PERIPH_IEEE802154_MODULE` clocks.
2. `ieee802154_rf_enable()` acquires PHY client bit
   `PHY_MODEM_IEEE802154 = 4` when the local RF guard is closed.
3. `esp_btbb_enable()` initializes BTBB on its first user, then increments its
   independent refcount.
4. `ieee802154_mac_init()` resets and configures the MAC.

The S31 clock dependency set is exactly
`802154_MAC | BT_I154_COMMON_BB | ETM | COEXIST | WIFI_BB_80X1 |
SOC_PLL_SOURCE_CG | BT_APB`.

Before dependency acquisition, `modem_clock_module_enable()` walks all ICG
domains and ORs the defaults into the current maps. `ACTIVE` is bit 2 (`4`) and
`MODEM` is bit 1 (`2`), producing these source-confirmed default additions:

| Domain | Added map |
| --- | ---: |
| MODEM APB | `6` |
| MODEM peripheral | `4` |
| Wi-Fi | `6` |
| BT | `4` |
| modem FE | `6` |
| IEEE 802.15.4 | `4` |
| LP APB | `6` |
| analog I2C master | `6` |
| coexistence | `6` |
| Wi-Fi power | `6` |

The IEEE 802.15.4 domain setter writes both the BT and ZB maps. The operation is
an OR with the pre-existing image, so unrelated map bits must be preserved.

Dependency bits are then visited from least to most significant. With the S31
feature gates, a fresh acquisition (all relevant refcounts transition `0 -> 1`)
has this physical order:

1. Enable the 160 MHz PLL source, then write
   `HP_SYS_CLKRST.MODEM_CONF = 0x3d`.
2. Set `MODEM_LPCON.CLK_CONF.CLK_COEX_EN = 1`.
3. Set `MODEM_SYSCON.CLK_CONF1.CLK_WIFIBB_80X1_EN = 1`.
4. Set `MODEM_SYSCON.CLK_CONF.CLK_ETM_EN = 1`.
5. Set `MODEM_SYSCON.CLK_CONF1.CLK_BT_APB_EN = 1`, then
   `MODEM_SYSCON.CLK_CONF.CLK_MODEM_SEC_APB_EN = 1`.
6. Set `MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN = 1`.
7. Set `MODEM_SYSCON.CLK_CONF.CLK_ZB_APB_EN = 1`, then
   `MODEM_SYSCON.CLK_CONF.CLK_ZBMAC_EN = 1`.

This is shared ownership, not a boolean clock switch. Each dependency normally
has a refcount; an enable only performs MMIO on `0 -> 1`, and a disable only
performs MMIO on `1 -> 0`. Disable visits the same low-bit-first order rather
than reversing it. An open driver must therefore acquire/release shared clock
leases and must not clear these fields unconditionally.

The current open ESP-HAL integration relies on the global clock tree to retain
the upstream `PLL_F160M` gate. Its IEEE transition separately reads back both
`REF_160M_CTRL0.REF_160M_CLK_EN = 1` and `MODEM_CONF = 0x3d`, fails closed if
either prerequisite is absent, and claims no refcounted release authority. A
future multi-client clock manager should replace that retained-gate contract
with an explicit lease.

At the beginning of MAC initialization, reset is performed under the modem
clock lock in this exact order:

1. `RST_ZBMAC`: write `1`, then `0`.
2. `RST_ZBMAC_APB`: write `1`, then `0`.

There is no delay or polling between either pulse in the reviewed source.

## 2. MAC foundation values and order

After the two reset pulses, the public source performs these operations in
order:

1. Initialize the software PIB. Defaults include channel 11, auto-ACK RX/TX
   enabled, enhanced-ACK TX enabled, promiscuous mode enabled, coordinator mode
   disabled, and RX-when-idle disabled. This only marks the PIB pending; it does
   not yet program `CHANNEL` or `TXPOWER`.
2. Clear the software TX/RX statistics.
3. OR `EVENT_EN` with `IEEE802154_EVENT_MASK = 0x3fff`. In non-test builds,
   clear timer-0 bit 8, giving an exact 14-bit field image of `0x3eff`; test
   builds retain `0x3fff`.
4. OR `TX_ABORT_EVENT_EN` with `0x01868000`, representing reasons 16, 18, 19,
   24, and 25.
5. OR `RX_ABORT_EVENT_EN` with `0x00028000`, representing reasons 16 and 18.
6. Assign `ED_CFG.ED_SAMPLE_MODE = IEEE802154_ED_SAMPLE_AVG = 1`.
7. Configure coexistence conditionally:
   - with software/external coexistence, call the coexistence ACK setter with
     `MIDDLE = 2` and the TX/RX setter for the idle scene (`IDLE = 4` by the
     public default configuration);
   - otherwise assign `PTI = 3` and `HW_ACK_PTI = 3` directly.
8. Call opaque `ieee802154_txon_delay_set()`.
9. Clear the software RX-buffer queue and set the software state to idle.
10. Allocate the IRQ and initialize sleep/retention support.

The abort-enable operations are OR assignments. The selected bits above are
proven to be set, but this source set does not document reset images for those
registers, so equality to those masks must not be claimed without an additional
register-reset source or HIL observation.

The pending PIB is applied later, immediately before an operation. Its hardware
write order is frequency code, TX-power index, CCA mode, CCA threshold,
auto-ACK/enhanced-ACK flags, coordinator/promiscuous flags, then pending mode.
The frequency-code part is fully source-confirmed; the TX-power-index part is
not, because its table provider is opaque.

## 3. Channel to frequency-code mapping

The accepted channel range is inclusive `11..=26`. The value written to
`IEEE802154.CHANNEL.FREQ` is not the channel number; it is:

```text
frequency_code = (channel - 11) * 5 + 3
```

Therefore the exact mapping is:

| Channel | Code | Channel | Code |
| ---: | ---: | ---: | ---: |
| 11 | 3 | 19 | 43 |
| 12 | 8 | 20 | 48 |
| 13 | 13 | 21 | 53 |
| 14 | 18 | 22 | 58 |
| 15 | 23 | 23 | 63 |
| 16 | 28 | 24 | 68 |
| 17 | 33 | 25 | 73 |
| 18 | 38 | 26 | 78 |

This mapping is suitable for a checked channel newtype. Values outside
`11..=26` must be rejected before the register write.

## 4. `EVENT_STATUS` clearing: INCOMPLETE

The LL helper named `ieee802154_ll_clear_events(events)` performs the volatile
bitfield RMW expression:

```c
IEEE802154.event_status.events &= events;
```

The ISR reads the current event image and passes that image to this helper
before dispatch. That sequence is consistent with write-one-to-clear hardware:
the RMW writes ones for selected events and zeroes for unselected events. It is
not consistent with ordinary RW clearing, and it would clear the wrong set on
write-zero-to-clear hardware.

This is still an inference, not an authoritative access-class statement. The
reviewed S31 register header supplies only address and mask, while the struct
header supplies only a writable-looking bitfield; neither labels
`EVENT_STATUS` W1C, W0C, read-only, or gives reset/concurrency semantics.
Consequently:

- lifecycle status for event clearing is **INCOMPLETE**;
- the PAC must not expose or use an ordinary `modify`-based clear as if RW were
  proven;
- IRQ ownership and event acknowledgement must remain outside the completed
  foundation state until a public TRM/SVD access annotation or controlled HIL
  experiment confirms the semantics.

A minimal HIL discriminator should latch at least two independent event bits,
acknowledge exactly one, and verify that the selected bit clears while the
unselected bit remains set. It should also inject an event between read and
write to check that acknowledgement does not lose a concurrent event.

The repository contains a validation-only target transaction and the
`ieee802154-event-status-selective-ack` HIL scenario for that discriminator.
The Iteration 9 form kept `EVENT_ENABLE` at zero, did not configure a CPU
interrupt route, used timer 0 and timer 1 as independent candidate stimuli,
failed closed on missing or contradictory timer-bit relations, and rejected
bits outside the public 14-bit field.

### Dated hardware observation: 2026-08-24

The Iteration 9 diagnostic image was flashed to a real ESP32-S31 and identified
by SHA-256
`841a496ad3f750a336c52289412533a0a10632f400fc86b5e786327e7949a63c`.
Two independent runs both stopped with `DualLatchTimeout`. Both returned zero
in every numeric `EVENT_STATUS` field retained by the Iteration 9 schema: the
reset sample, terminal dual-wait sample, and final cleanup sample. Post-ack and
distinct-arrival fields remained their zero initialization because the probe
stopped before those phases. That schema did not accumulate intermediate wait
samples or retain the cleanup-pending read. The setup required `EVENT_ENABLE`
to read back as zero at entry and did not write it or either CPU route.

This is a valid negative bounded-checkpoint observation, not W1C evidence. It
is compatible with `EVENT_ENABLE` gating status generation, but does not prove
that model; inactive timer counters or another unmet timer/clock precondition
would also explain the observation. It therefore neither confirms
acknowledgement semantics nor changes the access-class verdict from
**INCOMPLETE**.

The revised diagnostic image, SHA-256
`6398810693f171d970d589c0861e923420be4900d51f82f8e00bb0cab6aae3eb`,
added read-only timer-counter qualification and exact `EVENT_ENABLE` entry and
exit samples. Two subsequent reset-isolated runs produced the same bounded
trace:

- both `EVENT_ENABLE` samples were zero;
- each timer value started at zero, had observed minimum zero and maximum
  1,000, and read 1,000 after stop;
- the union of every initial bounded-wait `EVENT_STATUS` sample, its terminal
  sample, the cleanup-pending sample, and the final sample were all zero;
- each run stopped with `DualLatchTimeout` before any experimental acknowledge;
  later selective-ack and distinct-arrival fields remained their zero
  initialization.

This qualifies counter activity after both start commands and the
threshold-1,000 setup in the revised image. It removes inactive counters from
the live revised-stimulus alternatives, but does not retroactively observe the
Iteration 9 counters. The observation is consistent with `EVENT_ENABLE`
suppressing status generation or visibility, but does not distinguish those
mechanisms and is still not acknowledgement evidence. The probe did not change
`EVENT_ENABLE` or either CPU interrupt route, and the `EVENT_STATUS` access
class therefore remains **INCOMPLETE**.

The Iteration 11 diagnostic image, SHA-256
`8b0d34942f898047e0480a0a1f0240d65e1f7c2e0473a630d1286dfbf5d77336`,
added read-only source-132 route observations on both cores and enabled exactly
timer bits 8 and 9 only while every documented route-control field remained at
its reset value. Three reset-isolated boots passed the strict host validator.
Each boot established the following bounded trace:

- both complete route words retained exact zero before enable, while events
  were enabled, and after cleanup;
- `EVENT_ENABLE` changed from zero to the exact `0x0300` timer image and back
  to zero, with clear status before and immediately after enable;
- the simultaneous timer image was `0x0300`, followed by selective transitions
  `0x0300 -> 0x0200 -> 0x0000`;
- the distinct-arrival phase retained timer zero, introduced timer one, and
  then observed `0x0100 -> 0x0300 -> 0x0200` around the selected timer-zero
  write;
- both timer counters exhibited bounded activity, and the final masked status
  observation was zero.

This is positive evidence that the two selected raw writes are compatible with
selective W1C behavior under the exact detached-route experiment. It still
does not classify same-bit concurrent arrival, level-line retrigger, status
generation versus visibility gating, or whether a zero read after masking
proves the underlying latch physically clear. The production PAC therefore
keeps `EVENT_STATUS` observation-only, the active IRQ route remains absent, and
the overall access-class verdict remains **INCOMPLETE**.

The validation API consumes a process-lifetime reset-isolation capability and
the complete foundation owner. Its result is terminal: it exposes evidence but
cannot return to policy configuration or any operational transition. The
`EVENT_ENABLE` updates preserve every unreviewed upper register bit, while the
route safety gate rejects every nonzero bit in either complete raw route word.
These ownership constraints bound the experiment; they do not promote its raw
status writes into a production acknowledgement API.

## 5. Opaque calls that block an RF-ready claim

The following calls have declarations/call sites in the public source set but
no reviewed public implementation sufficient to reproduce their hardware
effects:

- `bt_bb_v2_init_cmplx(1)`: executed on the first BTBB user before the BTBB
  refcount is incremented. BTBB is explicitly required for IEEE 802.15.4.
- `register_chipv7_phy(...)`, `phy_wakeup_init()`, and `phy_close_rf()`: the
  public PHY client manager determines when to call them, but their RF
  calibration, wakeup, and shutdown register effects are outside the reviewed
  public implementation.
- `phy_param_track_tot(bool, bool)`: the reviewed `phy_track_pll()` scheduler
  determines when to call this leaf and whether Wi-Fi and/or the shared
  Bluetooth/IEEE 802.15.4 class is active, but the leaf's PLL hardware effects
  are outside the reviewed public implementation.
- `ieee802154_txon_delay_set()`: called during every MAC initialization after
  ED/PTI configuration and before the driver enters idle.
- `bt_bb_get_tx_pwr_table(&length)`: needed both to initialize each channel's
  default requested power and to convert dBm to the hardware TX-power index.
  The public weak fallback explicitly logs "not implemented" and returns null.
- `esp_coex_ieee802154_ack_pti_set()` and
  `esp_coex_ieee802154_txrx_pti_set()` when coexistence is enabled. The public
  header gives priority enums, but not the resulting register writes.

PHY ownership is structurally a client-set protocol, not a boolean. The public
enum assigns disjoint values Wi-Fi `1`, BT `2`, and IEEE 802.15.4 `4`;
`esp_phy_enable()` performs common RF bring-up only when the aggregate flag is
zero, then calls `phy_set_modem_flag(modem)`. `esp_phy_disable()` snapshots the
aggregate, calls `phy_clr_modem_flag(modem)`, and closes RF/common clocks only
when the saved value was exactly the caller's bit. The reviewed helper bodies
store a 16-bit software mask, set with bitwise OR, and clear with bitwise AND of
the complement after warning when the requested bit was absent. This is a
client set, not a per-client refcount; a correct open lifecycle must reject or
otherwise guard duplicate acquisition and release while preserving unrelated
client bits.

The reviewed PLL scheduler keeps separate previous timestamps for Wi-Fi and for
the shared Bluetooth/IEEE 802.15.4 class. Its immediate-on-enable path requests
tracking only when an active class has elapsed strictly more than the configured
period (1,000 ms by default), then passes the complete set of active classes to
opaque `phy_param_track_tot`. The periodic timer callback instead invokes the
internal tracker unconditionally and requests every active class without a due
check. With tracking enabled (the S31 default), first aggregate acquisition
creates and starts that timer before setting the caller bit; every acquisition
sets its bit before the immediate due check. Last-user release stops and deletes
the timer. The public source uses `ESP_ERROR_CHECK` for timer
create/start/stop/delete, so it supplies no recoverable failure owner or rollback
contract for a Rust implementation. Neither scheduler equivalence nor a set
client bit establishes that the opaque PLL adjustment, RF wakeup, or shutdown
effects completed.

The source obtains time separately for each enabled-class due check and again
for each timestamp refresh. A host model driven by one supplied time snapshot
can preserve the client-set and ordering rules, but it is not instruction-level
timing evidence at a tracking-period boundary; target execution and HIL must
cover that distinction.

Until the opaque effects above are replaced with reviewed source logic or
bounded HIL-backed contracts, the strongest honest endpoint is:

```text
ClockDependenciesAcquired -> MacReset -> FoundationConfigured
```

It must not be named `RfReady`, `ReceiveReady`, or `TransmitReady`.

## Next research gates

1. Extend the reviewed validation boundary to same-bit arrival during
   acknowledgement and active level-line interrupt retrigger. Separately
   distinguish event generation from masked visibility and prove whether a
   zero masked read establishes physical latch cleanup, or obtain an
   authoritative public `EVENT_STATUS` access classification for those
   semantics.
2. Establish a reviewed BTBB initialization contract for
   `bt_bb_v2_init_cmplx(1)`, including ownership and required postconditions.
3. Recover or replace the RF calibration/wakeup and `phy_param_track_tot`
   hardware effects behind the public PHY client manager. Reproduce the
   reviewed client-mask and tracking-scheduler protocol with explicit timer
   failure ownership, and test cold-start, warm-start, and already-owned paths
   separately.
4. Determine the exact registers and timing rule set by
   `ieee802154_txon_delay_set()`.
5. Establish a source-legal TX-power capability contract without copying an
   extracted vendor table; validate channel-specific dBm-to-index behavior on
   hardware.
6. Resolve coexistence-enabled PTI register effects, or explicitly qualify only
   the non-coexistence `3/3` path.
7. Before IRQ work, split task-owned MAC configuration from ISR-owned event
   status/enable access so one peripheral lease cannot silently cross ownership
   domains.
