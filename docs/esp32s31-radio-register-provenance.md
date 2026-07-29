# ESP32-S31 radio register provenance

This register pass separates three questions that were previously mixed in
raw constants:

1. Which MMIO address is accessed?
2. Which bit field exists on ESP32-S31?
3. Which initialization order and value has actually been exercised?

The SVD answers the first two for recovered radio blocks. HAL semantic
operations carry the third in comments; documented system blocks are decoded
only by the official PAC in the integration adapter.

## Clock and power chain

| Stage | Registers | Layout evidence | Operation evidence |
|---|---|---|---|
| HP-active modem ICG selection | `PMU.HP_ACTIVE_ICG_MODEM`, `IMM_MODEM_ICG`, `IMM_SLEEP_SYSCLK` | S31 SVD and PMU headers | pinned S31 `esp-hal` clock initialization |
| Modem bus and source | `HP_SYS_CLKRST.MODEM_CTRL0`, `MODEM_CONF` | official S31 PAC/SVD | pinned S31 `esp-hal` clock initialization |
| Domain power-state maps | `MODEM_SYSCON.CLK_CONF_POWER_ST`, `MODEM_LPCON.CLK_CONF_POWER_ST` | exact S31 structures and LL accessors | pinned S31 `esp-hal` values `0x64646400` and `0x66660000`, now composed from named fields |
| Wi-Fi BB reset and PHY gates | `MODEM_RST_CONF`, `CLK_CONF1`, `CLK_CONF`, LPCON `CLK_CONF` | exact S31 structures | pinned S31 `esp-hal` PHY/radio clock sequence |
| Frontend/baseband gate leaf | three `PHY_CLOCK_ORACLE` registers and `PMU.HP_ACTIVE_HP_CK_POWER` | mixed S31 PMU plus opaque instruction evidence | complete rev0 ROM `phy_open_fe_bb_clk` and blob `phy_close_fe_bb_clk` bodies |
| Analog PHY-I2C power/reset | `PMU.RF_PWC`, `IMM_HP_CK_POWER_0`, `ANA_PERI_PWR_CTRL` | official S31 PMU header and S31 SVD | complete `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new` body |

The former constant named `MODEM_LPCON_CLOCK_CONF_ADDRESS` at `0x2070401c`
was incorrect: that address is
`PMU.HP_ACTIVE_HP_CK_POWER`. Its bit 22 is the S31
`HP_ACTIVE_XPD_BB_I2C` field. The same ROM operation also sets bits 3:0; their
individual meanings remain unknown and are represented as
`ROM_OPEN_FE_BB_UNKNOWN_LOW`.

The three addresses previously labeled as MODEM_LPCON in the PHY port are
actually inside the S31 PMU block:

| Address | Recovered identity | Qualified fields |
|---|---|---|
| `0x207040f0` | `PMU.IMM_HP_CK_POWER_0` | all twelve S31 header fields; the open-I2C path uses `TIE_HIGH_XPD_BB_I2C` |
| `0x20704184` | `PMU.RF_PWC` | `XPD_RF_CIRCUIT[31:16]` |
| `0x20704208` | `PMU.ANA_PERI_PWR_CTRL` | `XPD_PERIF_I2C`, `RSTB_PERIF_I2C` |

The PMU header labels fields in `IMM_HP_CK_POWER_0` as `WT`, while the
complete S31 blob body loads the register before masking and storing it. CMSIS
SVD has no write-trigger access class. The official PAC fork at
`a633848ad` therefore models the register `read-write`, records the conflict
and blob source in its SVD patch, and lets the platform adapter preserve the
evidenced read/modify/write sequence. PMU is no longer described by the custom
radio SVD.

## PHY PBus and PHY-I2C

`PHY_MEMORY` at `0x20100800` now owns the shared table-memory aperture:

- `COMMAND` at `0x44`, with gain-mode's instruction-evidenced cleared bits
  10:0, the common eight-bit table index in bits 18:11, the shared
  gain-write/PBUS-command bit in bit 19, the final PBUS-command bit in bit 20,
  and the TX-CFR commit pulse in bit 21;
- three mode-dependent data words at `0x48..0x50`;
- six packed PBUS group-boundary words at `0x54..0x68`.

Complete rev0 ROM `phy_set_pbus_mem`, `phy_write_pbus_mem`, and
`phy_save_pbus_reg` prove the full 60-entry sequence and the twelve packed
`first/last` group pairs. Complete ROM `phy_write_gain_mem` and complete blob
`phy_set_tx_cfr_mem` prove that the command and data words are shared with
gain-memory and CFR modes. The SVD therefore exposes the common index and the
instruction-proven mode-dependent bits separately instead of assigning one
contradictory semantic identity to bits 20:11.

`PHY_CLOCK_ORACLE.TABLE_MEMORY_INDEX_SOURCE` at `0x20100408` completes the
chain. Complete rev0 ROM `phy_fe_reg_init` replaces its high byte with
`0xa0`; complete S31 `phy_set_tx_cfr_mem` and `phy_set_tx_gain_mem_new`
bodies each sample that byte once before their finite publication loops. The
lower 24 bits remain intentionally unnamed.

The public ESP32-S31 `esp-pacs` SVD pinned by this repository does not name
`0x20100844..0x20100868`. Public cross-chip PBUS memory-power fields are not
used to invent S31 identities for this internal aperture; complete S31
instructions remain the primary source.

The `phy_memory` HAL reproduces every recovered mode in exact order:

- PBUS memory: optional boundary RMW, data write, ten-bit command RMW;
- TX-CFR: data write, eight-bit index RMW, fresh-read commit set, fresh-read
  commit clear;
- gain memory: three ordered data writes followed by one RMW that clears bits
  10:0, replaces the index, sets bit 19, and preserves bits 31:20;
- front-end initialization: one typed high-byte base-index RMW.

PBUS, baseband CFR/RX-table, RX-gain and channel TX-gain bindings all borrow
`&mut RadioRegisters`. The former TX-gain C ABI with five raw input pointers
is gone: Rust explicitly concatenates the six seed words and eight packed
output words by checked slice indexing before publishing each of 32 entries.
No active publisher for this aperture contains a raw address or volatile
pointer.

`PHY_AGC_ORACLE` localizes the register set shared by complete rev0 ROM bodies
and complete pinned blob bodies:

- `phy_bb_agc_reg_update` at `0x2f82860e`, size `0xa6`;
- `phy_disable_agc` at `0x2f827460`, size `0x10`;
- `phy_enable_agc` at `0x2f827470`, size `0x28`;
- both branches of `phy_rx_11b_opt` at `0x2f827588`, size `0xc4`;
- `phy_agc_reg_init` at `0x2f8278d8`, size `0xd8`;
- both branches of `phy_rfrx_sat_rst` at `0x2f828944`, size `0x42`;
- `phy_pbus_force_mode` at `0x2f824102`, size `0x90`;
- `phy_ant_init` at `0x2f827df4`, size `0x44`;
- `libphy.a[phy_init.o]::phy_reg_update_new`, including complete
  `phy_set_ftm_en` and ROM `phy_wifi_agc_sat_gain` leaves;
- `libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table`, including its final two
  gain-limit writes;
- `libphy.a[phy_reg.o]::phy_set_rx_comp_new`, size `0x28`.

The internal electrical identities are not public. The SVD therefore records
the internal PHY registers with operation-scoped `OPAQUE`/`UNKNOWN`
names. It adds only instruction-proven fields: the disable bit, enable pulse,
six 11b fields, one cleared bit and one three-bit set field in the baseband
update, plus the post-init AGC, RX and FTM fields. The final three-bit
baseband update at `0x20109c18` is represented by the official S31 PAC's
`MODEM_SYSCON.WIFI_BB_CFG` field, alongside the unrelated PBus settle
condition. The custom radio SVD no longer duplicates this system peripheral.

The `phy_agc` HAL preserves the complete ROM access order: fourteen internal
baseband-update writes followed by one platform-PAC RMW, three fresh-read
edges when enabling AGC, one
fresh-read write when disabling it, and six ordered writes in either 11b
branch. Host register models cover the exact addresses, values, masks and
preservation of unrelated bits. Baseband initialization, RX-table
initialization and every open channel transition now borrow
`&mut RadioRegisters` for these operations; their former raw address blocks
have been removed from the live PHY crate.

The complete post-initialization sequence is now behind the same owned
boundary. `BLOB_LIBPHY_PHY_REG_UPDATE_NEW` records pinned `libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
and the complete rev0 saturation-gain leaf. The PAC adds five register
identities and reuses the instruction-proven nine-bit field on the shared
`RX_11B_WINDOW_CONTROL` word at `0x20107104`. The HAL preserves all seven
writes: one AGC RMW, two full-word saturation-gain stores, the shared window
RMW, two independently read RX-field RMWs, and the FTM-enable RMW. The
dynamic saturation-gain operation used by `phy_reg_init` is owned by the same
method. The former raw C ABI, raw pointers and duplicate bit-mask helpers are
removed; both live callers pass their unique `RadioRegisters` borrow.

The same PAC now owns AGC register initialization, both RF RX saturation
phases, and the final RX-gain limits. The SVD adds the complete fields used at
`0x201008bc`, `0x2010702c`, `0x2010705c`, `0x20107094`, `0x20107128` and
`0x2010713c`, plus the full-word phase configuration at `0x20107068`.
Discontiguous RF-saturation mask `0xd1080000` is represented by four
independently instruction-proven `UNKNOWN` fields rather than one invented
electrical name. Host models preserve the ten AGC-init updates, both
three-write saturation branches, both final limit writes and their fresh-read
ordering.

`configure_phy_registers`, `PhyBbMmioBinding`, and
`PhyRxGainInitMmioBinding` all pass their existing unique register borrow
into these safe methods. Raw access to `0x705c`, `0x7068`, `0x7094`,
`0x7128`, `0x713c`, and the parameter word `0x08bc` is now rejected by the
source-only audit.

The multifunction `0x702c` word is now fully behind that same PAC identity.
Complete `phy_set_rx_comp_new` proves the low-byte replacement there and the
high-byte replacement at `0x70a0`. Complete `phy_pbus_force_mode` proves its
high-byte replacement and set/delayed-clear pulse, plus the debug/work-mode
updates at `0x0884`, `0x088c`, and `MODEM_SYSCON.WIFI_BB_CFG`. Complete
`phy_ant_init` proves three fresh-read updates at `0x711c`, the independent
antenna field of shared `0x7030`, and `0x7120`. Unknown electrical meanings
remain explicitly `UNKNOWN`; independent consumers do not create duplicate
register identities.

The safe HAL preserves the two RX-compensation writes, the PBus tail's
high-byte/set/clear sequence, and all three antenna writes. Delay ownership
remains in the caller state machine. TX-DC, TX-DC/PWDET, PWDET, TX calibration
environment, RXIQ initialization, RX-gain DC, TXIQ, and RX-saturation MMIO
bindings now require the same unique `RadioRegisters` borrow. Their former
raw wrappers and duplicate mask helpers are removed. The source-only audit
now rejects raw `0x0884`, `0x088c`, `0x702c`, `0x7030`, `0x70a0`, `0x711c`,
and `0x7120` in addition to the earlier AGC addresses.

The channel cleanup tail is now fully capability-bound as well. Complete
pinned `libphy.a[phy_reg.o]::phy_dc_mem_clr`, size `0x1c`, proves a
fresh-read set/clear pulse on bit 20 of `0x703c`; the SVD v0.7 adds that field
without claiming an unevidenced electrical meaning. Complete rev0 ROM
`phy_bbpll_cal` at `0x2f827dbc`, size `0x1c`, proves the two encodings in bits
3:2 of official `I2C_ANA_MST.ANA_CONF0` at `0xf818`. That register
already owns the independently proven master-register enable and mode fields,
so no second alias is introduced.

The `phy_agc` HAL and platform `PhyI2cMasterControl` methods preserve the two
DC-memory reads and the single BBPLL RMW. Cold initialization, register
initialization, and channel changes borrow the platform-owned official
`I2C_ANA_MST` token for the latter operation. The raw C ABIs, address
constants, and duplicate mask helpers are deleted; the source-only audit
rejects raw `0x703c` and `0xf818`.

## Frequency and channel control

SVD v0.8 adds one `PHY_FREQUENCY_CHANNEL_ORACLE` identity for the thirteen
physical registers used by frequency-memory initialization and open channel
changes. The primary evidence is the pinned rev0 ROM, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`:

- `phy_freq_module_resetn`, `phy_freq_chan_en_sw`,
  `phy_freq_i2c_mem_write`, `phy_freq_reg_init`,
  `phy_freq_num_get_data`, `phy_freq_i2c_num_addr`,
  `phy_en_hw_set_freq`, and `phy_dis_hw_set_freq`;
- `phy_bb_bss_cbw40` and its digital child, `phy_mac_tx_chan_offset`,
  `phy_wifi_fbw_sel`, `phy_bt_filter_reg`, `phy_nrx_freq_set`,
  `phy_bb_cbw_chan_cfg`, `phy_wifi_enable_set`, `phy_mac_enable_bb`,
  `phy_bb_reg_init`, and `phy_i2c_master_mem_txcap`.

The complete pinned `libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
supplies the parent order through `register_chipv7_phy`, `phy_bb_init`, and
`phy_chip_set_chan`. Exact symbol searches in the public ESP-IDF tree expose
no definitions for these internal leaves. Public HT20/HT40 API terminology is
therefore not used to invent S31 electrical identities; fields remain
`UNKNOWN` unless a complete instruction body and its symbol establish more.

`FREQUENCY_CONTROL` at `0x2010001c` is deliberately modeled as one
multifunction word. Complete bodies prove the channel index in bits 7:0,
frequency-memory address in bits 18:8, channel-switch and memory-write pulses
in bits 19 and 20, initialization mode in bits 29:22, module enable in bit 30,
and active-low hardware-frequency ownership in bit 31. Bit 18 is also the
reset/release bit used by `phy_freq_module_resetn`; the PAC names that
mode-dependent collision instead of creating two aliases.

The remaining recovered layout includes:

- baseband mode and frequency-ready fields in shared parameter/status word
  `0x20100028`;
- a 24-bit frequency-memory payload plus eight-bit mode, two five-bit number
  addresses in the control word, and three words of six five-bit slots;
- the complete NRX twenty-bit quotient, cleared middle nibble, and preserved
  high-byte shift source;
- shared FBW/Bluetooth-filter fields, TX-offset and both CBW control words;
- Wi-Fi enable, BSS-CBW, MAC-baseband and cold-start fields on the official
  `MODEM_SYSCON.WIFI_BB_CFG` identity from PAC fork `a633848ad`;
- TX-cap publication through existing
  `PHY_I2C_COMMAND_RAM.COMMAND_MEMORY[1]`, whose block/register/data byte
  layout already describes `value << 16 | 0x026b`.

The native generated-PAC `RadioRegisters` methods preserve all fresh-read
edges, full-word constants, packed images and ROM branch encodings. Pure host
tests retain the NRX and CBW transforms, while target compilation verifies
the generated field accessors. The thin `phy_frequency` HAL performs no raw
or compatibility-register access: cold init, baseband init and channel
actions coordinate the platform capability with the same
`&mut RadioRegisters`; the D-code MMIO
binding is no longer `unsafe` and cannot manufacture a second peripheral
owner. The source-only audit rejects all raw accesses to `0x001c..0x003c`,
`0x0874`, `0x4400`, `0x7848`, `0x7ce0`, `0x7ce4`, `0x9c18`, and `0xfc04`,
as well as the removed wrapper names.

`PHY_PBUS.STATUS_CLOCK_FORCE` at `0x20100890` is now confirmed as one physical
multifunction register rather than conflicting guessed aliases. Independent
complete rev0 ROM bodies establish:

- bits 11:8: the four-bit value replaced by `phy_force_txrx_off`;
- bits 15:14: the pair controlled by `phy_set_rxclk_en`;
- bits 17:16: the pair controlled by `phy_set_txclk_en`;
- bit 31: the busy status sampled by `phy_pbus_force_test`.

The pair-level functions do not distinguish the two constituent clocks. The
SVD therefore keeps the TX pair as `TX_CLOCK_ENABLE_PAIR`. RX bits 14 and 15
also have independent RXIQ-root consumers, so they are represented as two
non-overlapping multifunction fields whose names retain both the clock and
unknown-status roles. The four force encodings are exact, but their electrical
meaning is still unknown and remains `FORCE_TXRX_MODE_UNKNOWN`.

The complete `phy_pbus_rd` address and shift tables expose five packed result
words at `0x20100894..0x201008a4`. Each visible nine-bit window is represented
without assigning an analog meaning. Only selector 1's low window has a
qualified consumer identity: the RX-DCO calibration path.

The official ESP32-S31 PAC records both host command words, the master control
fields, read-mask callback, opaque 14-bit host map and three clock-selection
words. The custom radio PAC records the 45 command-memory entries. Command
layout is instruction-exact:
block/register/data occupy bytes 0/1/2, followed by write, busy and
start/reset bits 24/25/26.

The corresponding HAL is split by hardware ownership:

- `analog_i2c` owns PMU power/reset sequencing;
- `pbus` owns command publication, completion sampling, packed reads and the
  RX/TX clock pairs;
- platform `PhyI2cMasterControl` owns official `I2C_ANA_MST` host commands,
  the six-write clock-selection transform and master setup;
- `phy_i2c` retains only bounded custom PHY-I2C command RAM.

Every public operation documents both its S31 register-layout source and the
complete ROM/blob body used for operation order. The cold PHY binding borrows
the platform capability for analog-I2C and `&mut RadioRegisters` only for
custom radio blocks. Reusable RFPLL, RXIQ/TXIQ, DCO, gain, temperature,
saturation, power and power-detector target bindings use the corresponding
explicit capability; no raw-owner PHY-I2C or PBus force-test leaf remains.

## Baseband initialization and power-detector PAC

SVD v0.9 adds the 36-register `PHY_BASEBAND_CONFIG_ORACLE` aperture and the
independently addressed `PHY_POWER_DETECTOR_AUX_ORACLE` register. This is the
first complete typed representation of the local MMIO used by
`phy_reg_init`, its baseband/watchdog/PA/noise leaves, TX-power tracking, and
the PWDET/TX-DC calibration path.

The address, mask, value and access-order sources are pinned S31 artifacts:

- rev0 ROM ELF SHA-256
  `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
  including complete `phy_reg_init` (`0x2f823ef8`, size `0x52`),
  `phy_bb_reg_init` (`0x2f8279c6`, size `0x140`),
  `phy_tx_paon_set` (`0x2f82764c`, size `0x78`), and the complete PWDET
  leaves recorded in the SVD source ledger;
- pinned `libphy.a` SHA-256
  `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
  including complete `phy_bb_txpwr_track`, `phy_txgain_comp_pacfg_new`, and
  `phy_txdc_cal_pwdet_init`.

The new safe `phy_baseband` and `phy_power_detector` HAL modules preserve
each full-word store and each separate fresh-read update. This includes
apparently redundant operations such as the second `0x7cd0` OR and the
individual PWDET clears; they are not folded into a guessed equivalent
transaction. Host register models assert operation counts, order, captured
field restoration, and final images.

Cold PHY init and the reusable BB/PWDET/TX-DC target bindings now require the
same `&mut RadioRegisters` capability for configuration and sampling. In
particular, ready/result reads at the PWDET control/result identities no
longer manufacture global volatile pointers.

SVD v2.1 removes `PHY_POWER_DETECTOR_AUX_ORACLE`. Its sole address,
`0x2070_1068`, is exactly the official
`LP_AON_CLKRST.RTC_SAR2_PWDET_CCT` register and field. The platform adapter
now owns the `LP_AON_CLKRST` singleton and exposes only the two complete-ROM
encodings: four for `phy_pwdet_reg_init`/`phy_pwdet_sar2_init`, and two for
`phy_txcal_debuge_mode_`. The official PAC patch records those same ROM
sources, while the open PHY crate retains their required operation order.

SVD v2.2 removes `PHY_TEMPERATURE_SYSTEM_ORACLE`. Its sole register is
exactly official `LP_PERICLKRST.TSENS_CTRL`, and complete pinned
`libphy.a[phy_tsens.o]::phy_tsens_read_init` identifies bit 30 as the LP
temperature-sensor clock enable. The platform adapter owns the `LP_PERI`
singleton; the open HAL retains the five-edge blob order around that official
PAC operation.

SVD v2.3 removes `PHY_TEMPERATURE_SENSOR_ORACLE`. Its two registers are
official `LP_TSENS.CTRL` and `LP_TSENS.CLK_CONF`. The official PAC patch
records the complete blob/ROM sources for the three read-path edges, sensor
power and low-byte code sample. Both cold initialization and reusable
temperature sampling now require the platform-owned `LP_TSENS` singleton.

SVD v2.4 removes `PHY_I2C_MASTER` at `0x2010_f800..0x2010_f82f`. This aperture
is exactly the official `I2C_ANA_MST` peripheral. The ESP32-S31 PAC patch
records the pinned rev0 ROM and `libphy.a[phy_i2c.o]` sources for host command
publication, bit-25 busy sampling, bit-26 start/reset, read-mask programming,
master mode/enable, BBPLL calibration and the three clock-selection words.
The platform adapter owns the official singleton and implements only semantic
operations while preserving each separately observed fresh-read/RMW edge.
The adjacent undocumented command memory at `0x2010_fc00` remains in the
custom PAC as `PHY_I2C_COMMAND_RAM`; it is not part of the official
`I2C_ANA_MST` address block.

Public Espressif sources do not currently define these S31 internal PHY
fields. The public
[ESP32 Open MAC project](https://github.com/esp32-open-mac/esp32-open-mac)
likewise treats hardware initialization as a remaining blob boundary, while
the open-driver
[static-analysis paper](https://arxiv.org/abs/2501.17684) explains why MMIO
semantics cannot always be inferred from address traces alone. Those sources
support the conservative naming policy, not cross-chip field names:
instruction-proven roles are named, and unresolved electrical meanings remain
`UNKNOWN`.

## IQ estimator and shared activity PAC

SVD v1.0 adds the eleven-register `PHY_IQ_ESTIMATOR_ORACLE` block. It owns
the DC/IQ configuration and control words, four signal-power results, three
DC/power accumulators, readiness status, and the activity word shared with
RX-saturation sampling.

The register and field sources are complete pinned artifacts:

- rev0 ROM ELF SHA-256
  `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
  including complete `phy_iq_est_enable` (`0x2f8289d4`, size `0xb4`),
  `phy_iq_est_disable` (`0x2f828a88`, size `0x2c`), `phy_dc_iq_est`
  (`0x2f828ab4`, size `0x84`), `phy_rxiq_get_mis` (`0x2f828b84`, size
  `0x13e`), `phy_set_rx_gain_cal_iq` (`0x2f82964c`, size `0x20c`), and
  `phy_get_rx_sig_pwr` (`0x2f829ea2`, size `0x76`);
- pinned `libphy.a` SHA-256
  `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
  including complete `phy_rx_cal.o::phy_check_rx_sat`, size `0x76`.

The safe `phy_iq_estimator` HAL retains the three setup reads, independent
start/measurement enable edges, and all signed result reads. The two
four-word consumers deliberately have different methods because their
complete ROM bodies have different physical read orders:
`phy_rxiq_get_mis` reads sum-I, sum-Q, difference-Q, difference-I, whereas
`phy_get_rx_sig_pwr` reads sum-I, sum-Q, difference-I, difference-Q. The old
raw struct-construction order did not preserve either sequence even though
the resulting field association was correct for stable registers.

The activity register has one PAC identity for both uses. Estimator
readiness samples and every one-shot `phy_check_rx_sat` sample now require
the caller's unique `&mut RadioRegisters` capability. The transition still
owns the bounded 100-sample policy; the HAL performs exactly one finite read.

The live implementation no longer reaches these identities through the
handwritten `Register32`/`Field32` compatibility facade. Configuration,
enable edges and observations are native generated-PAC operations. Their
source retains the three separate configuration RMWs and the two distinct
four-word ROM read orders; the HAL is limited to converting PAC tuples into
named semantic snapshots. The SDM deadline counter used by the same cold
prelude is likewise one generated-register read with no test-only duplicate
register identity.

## Temperature-sensor PAC

SVD v1.1 adds the two-register `PHY_TEMPERATURE_SENSOR_ORACLE` aperture and
the independently addressed `PHY_TEMPERATURE_SYSTEM_ORACLE` control word.
The shared `SENSOR_CODE_POWER` identity is deliberately one register: its
low byte is the unsigned sensor code, while bit 22 is the power field.

The instruction sources are the same pinned artifacts used by the live cold
initializer:

- `libphy.a` SHA-256
  `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
  including complete `phy_tsens.o::phy_tsens_read_init`, size `0x36`, and
  its complete `phy_set_tsens_power`, size `0x1c`;
- rev0 ROM ELF SHA-256
  `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
  including complete `phy_set_tsens_power_` (`0x2f825dc8`, size `0x1c`),
  `phy_tsens_code_read` (`0x2f825ee0`, size `0x0c`), and
  `phy_tsens_temp_read_local` (`0x2f825f1e`, size `0x5e`).

The safe `phy_temperature::initialize` HAL method retains all five separate
fresh-read updates: sensor-control bit 0, system-control bit 30,
sensor-control bit 23, sensor-control bit 9, then power bit 22. The field
names for the three read-control bits and the system bit remain `UNKNOWN`
because the bodies prove masks and order, not their electrical roles.

Temperature sampling is now a semantic, address-free PHY action. Its
non-cloneable binding requires `&mut RadioRegisters`, and the HAL extracts
the PAC `CODE` field from exactly one read. The former raw addresses, mask,
volatile wrappers, and duplicated power-field test are removed. The
source-only audit rejects their return.

## RX-DCO control PAC

SVD v1.2 adds the `PHY_RX_DCO_ORACLE.CONTROL` identity at physical address
`0x2010_0434`. Only bits 23:22 are described. Their electrical role is not
proved by the available instruction bodies, so the PAC deliberately retains
the `CALIBRATION_CONTROL_UNKNOWN` name.

Two complete pinned bodies independently prove the same save, clear and
restore sequence:

- rev0 ROM ELF SHA-256
  `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
  `phy_pbus_rx_dco_cal` at `0x2f82_8f44`, size `0x228`;
- `libphy.a` SHA-256
  `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
  `phy_rx_cal.o::phy_xtal_duty_cal`, size `0x392`.

Both bodies first read and retain bits 23:22, perform another fresh read before
clearing them, and use one final fresh read to restore only the saved field.
The new safe `phy_rx_dco` HAL preserves those separate read/modify/write
edges. Its capture returns the field in its original bit position, while
restore masks untrusted high bits and preserves every unrelated current
register bit.

The RX-DCO, RX-DC calibration, RX-gain initialization, crystal-duty and cold
initializer bindings now pass the caller's unique `&mut RadioRegisters`
capability to that one HAL owner. Their repeated raw volatile helpers were
deleted. The adjacent duplicated raw `phy_pbus_rd` address/shift table was
also removed: all three live calibration consumers now use the existing safe
PAC-backed PBus result reader. The source-only audit rejects the old helper
names and any new raw `0x2010_0434` literal in the live PHY crate.

## Cold-PHY prelude and deadline PAC

SVD v1.3 completes the register ownership boundary around the early
`register_chipv7_phy` prelude. Three identities were already present in the
recovered PAC and now have their live consumers moved behind safe HAL methods:

- `PHY_PBUS.STATUS_CLOCK_FORCE.FORCE_TXRX_MODE_UNKNOWN` owns bits 11:8 used
  by the two force and two release phases;
- official `I2C_ANA_MST.I2C0_CTRL/I2C1_CTRL` own the reset command at bit 26
  and the sampled busy state at bit 25;
- official `MODEM_LPCON.TICK_CONF.MODEM_PWR_TICK_TARGET` owns the six-bit
  fixed-crystal tick target; the open driver exposes only the semantic
  platform operation.

The new `PHY_COLD_DEADLINE_ORACLE.DEADLINE_COUNTER_UNKNOWN` identity owns the
full read-only word at `0x2010_d800`. Complete rev0 ROM
`phy_wait_i2c_sdm_stable` (`0x2f82_3e76`, size `0x4a`) proves its physical
address, full-width reads, and wrapping unsigned deadline use. The ROM artifact
SHA-256 is
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
The counter's clock source is not proved, so the PAC deliberately does not
assign one.

The other finite operations are grounded by complete pinned bodies:

- rev0 ROM `phy_force_txrx_off` at `0x2f82_7bb0`, size `0x66`, proves the
  ordered field encodings `8`, `10`, `2`, and `0` with a fresh read before
  every write;
- rev0 ROM `phy_i2c_master_reset` at `0x2f82_60d0`, size `0x74`, proves both
  host addresses, reset command, and busy bit;
- pinned `libphy.a` SHA-256
  `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
  complete `phy_init.o::phy_get_xtal_freq`, size `0x40`, proves the
  `frequency_mhz - 1` field transform. The public S31 contract fixes this
  input at 40 MHz, so the HAL writes 39 without consulting hidden RTC state.

The safe HAL performs only one finite edge per call. Delay order, reset
retries, and the inclusive 9,999-cycle deadline remain explicit state-machine
state. Reset sampling crosses the boundary as `busy: bool`; neither a physical
address, a mask, nor a raw register word appears in the PHY action/completion
protocol. The old raw wrappers and constants are deleted, and the source-only
audit rejects their names plus raw `0x2010_d800`, `0x2010_f028`,
`0x2010_f800`, and `0x2010_f804` literals in the live PHY crate. At this
revision the shared `0x2010_0890` word still had separately evidenced RXIQ
status consumers on the remaining raw frontier; SVD v1.4 below closes that
frontier.

SVD v1.4 localizes the final live consumers of the shared
`PHY_PBUS.STATUS_CLOCK_FORCE` word. The primary new source is pinned
`libphy.a` SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`,
complete `phy_rx_gain.o::phy_rxiq_cal_init`, size `0x198`. Its root entry
independently sets bit 14 and then bit 15 with a fresh read before each write.
Its successful cleanup independently clears bit 15. Together with complete
rev0 ROM `phy_set_rxclk_en`, this proves that the two physical bits have both
a pair-clock consumer and distinct RXIQ-root consumers. The electrical status
meaning remains unknown, so the PAC does not invent a narrower semantic name.

The same blob body proves the exact correction prefix and suffix on
`PHY_BASEBAND_CONFIG_ORACLE.IQ_CORRECTION_CONTROL` and
`IQ_CORRECTION_AUX`. Prefix order is:

1. set control bit 29;
2. set auxiliary bit 13;
3. clear control bit 30;
4. clear auxiliary bit 14.

The successful suffix sets control bit 30, sets auxiliary bit 14, clears
control bit 29, and finally clears the shared status/clock bit 15. Every step
uses a new hardware read. The previous raw Rust prefix combined the two
control updates and the two auxiliary updates, producing only two RMW edges.
The safe HAL now preserves all four prefix and all four suffix edges; a fake
register test records the two root-status writes plus those eight correction
writes in exact order.

Because CMSIS-SVD fields cannot overlap, the former two-bit RX and TX
correction fields are represented as disjoint low/high single-bit fields.
The complete ROM initialization leaf still writes each pair in one RMW,
whereas the RXIQ parent deliberately uses separate RMW calls. All clock,
root-status and correction target bindings now require the same unique
`&mut RadioRegisters` capability. The raw wrappers are removed, and the
source-only audit now rejects both their names and raw `0x2010_0890` from the
live PHY crate.

## RX-gain DC calibration control

SVD v2.5 localizes the remaining direct-register prefix and cleanup of
`phy_set_rx_gain_cal_dc`. The primary source is complete rev0 ROM
`phy_set_rx_gain_cal_dc` at `0x2f82_9858`, size `0x206`, from the pinned ROM
ELF named above. Its prefix reads `0x2010_0424`, sets bits 6:5 with mask
`0x60`, and writes the result before entering the bounded RX-gain DC
calibration graph. The common cleanup tail performs a fresh read of the same
word, clears `0x60`, and writes the result.

The SVD therefore names the word `RX_GAIN_DC_CONTROL` and retains
`CALIBRATION_ENABLE_UNKNOWN` for bits 6:5 because the instruction stream does
not independently prove a narrower electrical meaning. It records only the
two values actually established by the complete control flow: `ENABLED=3`
and `DISABLED=0`. The PAC method performs exactly one fresh RMW per call.
Calibration-clock enable remains a preceding, separately owned operation only
on the prefix path, matching ROM order.

This leaf is consumed directly through the generated `svd2rust` register API;
it is deliberately absent from the handwritten `Register32` compatibility
facade. The PHY wrapper is now safe and requires `&mut RadioRegisters`. The
former `PHY_TONE_SELECTOR_CONTROL_ADDRESS - 4` expression is deleted, and the
source-only audit rejects that derived address, the raw literal, and an unsafe
version of the wrapper.

## ADC-rate and front-end register cluster

SVD v2.6 moves three complete leaves directly onto the generated PAC. Primary
sources are the pinned rev0 ROM ELF's complete `phy_adc_rate_set` at
`0x2f82_a6d2`, size `0x4a`, complete `phy_fe_reg_init` at `0x2f82_7740`,
size `0xf6`, and pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`, size
`0x32`. Their hashes and source identities are recorded in the SVD ledger.

`phy_adc_rate_set` performs its separately owned PHY-I2C subgraph first, then
uses two fresh reads of `0x2010_0448` to publish rate bit zero into physical
bit one and physical bit zero. The PAC method preserves those two edges and
the caller passes the existing unique `&mut RadioRegisters` capability.

The complete front-end initializer has seventeen ordered writes. Four form a
prefix, the fifth configures the already owned table-memory base index, and
the remaining twelve form the suffix. The PAC exposes prefix and suffix
methods so the PHY wrapper can retain that intervening typed table-memory
operation without raw MMIO or a second register owner. Repeated writes to
`0x2010_0448` and combined updates to the shared IQ-control words remain
exactly as instructed by ROM.

The pinned front-end update is deliberately the three-write archive body:
two separate sets at `0x2010_0c08`, then one combined set of bits 1:0 at
`0x2010_0448`. It does not acquire the similarly named ROM function's extra
DAC-scale tail. The generated PAC now owns unique words at `0x2010_0444`,
`0x2010_0448`, `0x2010_086c`, `0x2010_0894`, `0x2010_0c08`, and
`0x2010_0c20`; the source-only audit rejects those literals in the PHY crate.
Shared `0x2010_040c`, `0x2010_0438`, and `0x2010_0c0c` identities remain
single generated register objects. Their later tone/IQ consumers are now
also ownership-threaded through those same objects.

## TX-DC measurement and comparator control

SVD v2.7 describes `TX_DC_MEASUREMENT_CONTROL_STATUS` from complete rev0 ROM
`phy_txdc_cal` at `0x2f82_abbe`, size `0x1dc`, in the pinned ROM ELF. The
instruction stream proves measurement start and enable at bits 0 and 1,
readiness at bit 22, and the separately sampled Q and I comparator results at
bits 28 and 29.

The generated PAC preserves the three trigger RMW edges, one read per ready
poll, two independent comparator reads, and the two cleanup RMW edges.
Higher-level actions no longer expose a physical address, mask, expected
register image, or raw comparator words: ready and comparator observations
cross the ownership boundary as booleans. The ready binding now also borrows
`&mut RadioRegisters`, removing the last unowned access to this word. The
source-only audit rejects the raw address, former constants and unsafe wrapper
signatures.

## IQ correction ownership

The already described `IQ_CORRECTION_CONTROL` and `IQ_CORRECTION_AUX`
registers now have native generated-PAC operations for the remaining mode and
coefficient consumers. Primary sources remain complete rev0 ROM
`phy_txiq_set_reg`, `phy_rxiq_set_reg`, and the complete TXIQ/RXIQ calibration
parents recorded in the SVD provenance.

Entry mode updates preserve their combined clear/set RMW, completion preserves
its single-bit set, and signed gain/phase values are truncated to the
instruction-proven six- or seven-bit fields inside the PAC. TXIQ and RXIQ
bindings now carry the unique `&mut RadioRegisters` borrow. The raw
`0x2010_0438`/`0x2010_0c0c` constants and upper-layer coefficient transforms
are deleted and rejected by the source-only audit.

The common register-initialization enable leaf and complete RXIQ root
status/prefix/suffix sequence now use these same generated objects directly.
The two PBus status publications and all eight correction-mode updates remain
separate fresh-read edges exactly as in the complete pinned body; the former
HAL mask model and its duplicated status-register identity are removed.

All remaining baseband-configuration leaves now use the same native boundary:
TX-power tracking, I²C TX rate and gain compensation, watchdog, automatic
noise-floor control, PA-on setup and the complete local portion of
`phy_bb_reg_init`. Generated field readers preserve the instruction-exact
partial clears inside the two multi-bit initialization fields. The HAL module
contains only the sequencing split around NRX and official-platform control;
it no longer imports `Register32`, `Field32`, masks or compatibility MMIO.

The first AGC slice is native as well. Complete ROM enable/disable edges,
pinned RX-compensation writes, the DC-memory pulse, and the two PBus
work-mode pulse segments now operate on generated fields. The one- and
two-microsecond delays remain in the caller state machine; moving MMIO into
the PAC does not collapse those asynchronous hardware boundaries.

All remaining AGC consumers now use native generated access too:
`phy_bb_agc_reg_update`, `phy_agc_reg_init`, antenna setup, both RF-RX
saturation branches, final gain limits, saturation-gain stores,
`phy_reg_update_new`, and both `phy_rx_11b_opt` branches. Instruction-exact
full-word constants are localized in the PAC next to the source citations;
HAL no longer carries an AGC register model or numeric masks.

## Calibration-tone ownership

SVD v2.8 describes the complete tone cluster used by PWDET, TX-power, TX-DC,
TX-IQ, RX-IQ and crystal-duty calibration. Primary local sources are the
pinned `_oracles/esp32s31_rev0_rom.elf` bodies
`phy_start_tx_tone_step`, `phy_stop_tx_tone`, `phy_dac_scale_set` and
`phy_txiq_get_mis_pwr`, plus
`_oracles/libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` (size `0xc2`) and
`phy_txgain_comp_pacfg_new` (size `0x54`).

The generated PAC now owns:

- the shared tone-stop field in `0x2010_040c`;
- ordered TX-gain compensation writes at `0x2010_0410` and the full zero
  auxiliary write at `0x2010_0414`;
- both packed tone path words at `0x2010_041c` and `0x2010_0420`, including
  selector-high, negated step/attenuation, arm/enable and recovered TX-IQ
  mismatch images;
- the two separately written selector-low fields at `0x2010_0428`;
- both separately written DAC-scale bytes at `0x2010_0c04`.

The PAC preserves the complete source operation order and all fresh-read RMW
edges. Full-word TX-IQ save/restore is safe at the PHY boundary because the
saved image is sampled and restored through the same unique
`RadioRegisters` owner; its necessary generated writer `unsafe` is local to
the PAC with that invariant documented.

All former address constants and raw volatile helpers are removed from
`open-esp-radio-phy-esp32s31`. Tone actions now borrow
`&mut RadioRegisters`, and the final removal of obsolete `unsafe` markings
from every PHY target binding makes the entire upper PHY crate safe. The
source-only audit rejects all seven physical tone literals, former wrapper
signatures, and any future `unsafe` or raw volatile access in the PHY crate.

## Native power-detector PAC operations

The already described `POWER_DETECTOR_CONTROL`,
`POWER_DETECTOR_SAR_CONTROL_STATUS`, table, reference and result registers no
longer pass through the handwritten `Register32` facade. Their complete rev0
ROM and pinned `libphy.a[phy_tx_cal.o]` sequences now execute directly on the
generated register objects held by `RadioRegisters`.

The native PAC preserves the three independently fresh enable-bit clears,
the two independent TX-DC capture reads followed by full preserved-image
writes, the clear/set SAR trigger edges, and all separately ordered field
restores. Pure PAC tests retain the instruction-derived enable and TX-DC image
checks that previously lived in a fake compatibility-register model.
Platform-only LP-AON mode selection remains in the official PAC adapter and
is sequenced by the thin HAL wrapper. The source audit rejects any return of
`Register32`, `Field32` or compatibility read/write methods to this module.

`PHY_RX_DCO_ORACLE.CONTROL` has independently moved to the same native API.
The PAC samples the saved field, performs the complete source's second fresh
read while clearing it, and restores a caller-carried encoded image through
one generated-field RMW. Pure tests pin the bits-23:22 encode/decode boundary;
the HAL no longer imports compatibility register or field types.

The `PHY_MEMORY` aperture and shared table base-index field have also moved to
native generated access. PBUS boundary updates, data-before-command order,
the two-edge CFR commit pulse, three-word gain publication and six ordered
boundary captures are now methods on the unique `RadioRegisters` owner.
`PbusMemoryGroupBoundary` and `PhyMemoryError` are PAC-owned semantic types
re-exported by HAL, so existing PHY callers keep their source-level API
without retaining a second mask/shift implementation.

## Native MAC BlockAck access

The generated `WIFI_MAC_RX_DMA` peripheral is now the only live register
access path for the receive-agreement transaction and completed-TX BlockAck
sampling. Primary evidence remains the complete pinned `libpp.a` BlockAck
leaves plus the corresponding
`migration/esp32s31-hybrid-runtime/src/{rx_ampdu_hw,tx_ampdu}.rs`
transcriptions already cited by the SVD.

The PAC keeps the receive index selection, entry publication, commit pulse,
readback latch, diagnostic reads and fences as distinct operations. It also
matches the four descending TX queue banks explicitly, avoiding handwritten
address arithmetic. The upper MAC modules accept semantic values or decoded
register images and no longer import the handwritten register facade for
these leaves.

The same generated peripheral now owns the live RX descriptor-walker
transaction. Complete rev0 ROM `wDev_AppendRxBlocks`,
`hal_mac_rx_enable`, `hal_mac_rx_disable`, and last-descriptor sampling,
together with `HIL_OPEN_RX_LIVE_APPEND_2026_07_27`, remain the primary
evidence already attached to the SVD registers. The upper ring sees only a
semantic `RxDma` capability; raw identities are no longer part of its API.
Host tests retain the exact read/write/fence trace through a semantic model.

The hard MAC interrupt transaction is now a separate generated
`WIFI_MAC_INTERRUPT` peripheral at `0x2010_4c40`. Complete
`libpp.a::hal_mac_interrupt_get_event` reads masked status at
`0x2010_4c48`; complete `hal_mac_interrupt_clr_event` writes the sampled
image to the write-to-clear word at `0x2010_4c4c`. Recovered
`wDev_ProcessFiq` supplies the known TX-complete, BSS-color-collision,
watchdog, RX-success and TX-timeout bit identities recorded in the SVD. The
upper ISR receives only a semantic snapshot/acknowledge capability and cannot
name or manufacture an MMIO register.

The hardware CCMP transaction is likewise split into generated
`WIFI_MAC_CRYPTO_CONTROL` at `0x2010_4800` and `WIFI_MAC_KEY_TABLE` at
`0x2010_5800`. Its primary source is the complete `hal_crypto.o` recovered
from pinned `_oracles/libpp.a` SHA-256
`f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`:
`hal_crypto_clr_key_entry` proves 25 validity bits, ten ordered word clears
and the `0x28`-byte entry stride; `hal_crypto_set_key_entry` proves the
peer/control/key publication; `hal_crypto_is_key_valid` proves the validity
readback; and the reachable STA/CCMP branch of `hal_crypto_enable` proves the
interface and policy sequence.

The SVD deliberately leaves partly decoded control and policy fields named
`UNKNOWN`. Their exact write images and masks are instruction-exact, but the
individual electrical meanings are not yet independently identified. The
existing migration STA scan/WPA/DHCP qualification supports the complete
transaction but is not treated as independent register-level HIL evidence.
The upper MAC therefore receives only a semantic install/clear capability;
all table geometry and generated register access remain inside the PAC.

Ordinary EDCA TX is described as four generated blocks rather than one guessed
monolith. `WIFI_MAC_TX_COMMON` contains shared CCA and queue
timeout/completion state. `WIFI_MAC_TX_QUEUE_CONTROL` contains four ascending
`0x10`-byte banks; `WIFI_MAC_TX_QUEUE_VECTOR` and
`WIFI_MAC_TX_COMPLETION` contain four ascending `0x7c`-byte banks. In all
three cases physical order is logical queue 3,2,1,0, and the PAC performs the
single reversal at its semantic boundary.

Primary evidence is pinned `_oracles/libpp.a` SHA-256
`f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`:
complete `hal_mac_tx_config_timeout`, `hal_mac_tx_set_ppdu`,
`hal_mac_tx_config_edca`, `hal_mac_txq_enable`, `hal_mac_txq_disable`,
`hal_mac_get_txq_state`, `hal_mac_clr_txq_state`, and `mac_tx_set_pti`
leaves, together with the recovered completion reader. The former migration
runtime in the parent of promotion commit `f233006` preserves the exact
address/stride and async timeout/completion transcription. Open
authentication and connected WPA2 HIL qualify queue zero; the other three
ordinary banks remain instruction-exact but have not been independently
exercised by the open driver.

The complete management-TX chain proves that the queue scheduler priority and
the packet PTI are separate values. Complete
`libnet80211.a[ieee80211_output.o]::ieee80211_send_probereq` assigns active
Probe Request event 5. Complete `ieee80211_send_mgmt` assigns Authentication
and Association event 6 through `ieee80211_set_tx_pti`, then submits through
`ieee80211_mgmt_output`. Complete
`libcoexist.a[coexist_core.o]::coex_pti_tab` maps both events 5 and 6 to packet
PTI 1 and event 1 to PTI 5. Complete
`libpp.a[hal_mac.o,hal_coex.o]::{mac_tx_set_pti,hal_set_tx_pti}` retains the
original packet value 1 in all four vector lanes, selects the independent
queue scheduler value as unsigned `min(1, 5) = 1`, and publishes count 1.
The `bltu packet_pti,event_one_pti` branch at complete `mac_tx_set_pti+0x44`
is the decisive instruction: it skips replacement when the packet value is
already smaller.

The earlier open program incorrectly wrote one value into both locations.
SVD v3.23 names bits 31:28 of the queue configuration
`SCHEDULER_PRIORITY`, while the vector register retains the four `PTI_n`
fields. The MAC/PAC boundary now represents the two values independently. On
connected ESP32-S31 rev0 HIL, open Authentication read back queue
configuration `0x520013ff` and PTI vector `0x00111110`, then completed
Association, WPA2 and DHCP. The scheduler value five in that run was an open
experiment based on the earlier reversed interpretation of the comparison,
not a vendor snapshot. Five reset-to-DHCP repetitions all connected, but only
two of five received the first Authentication response under the former
100-ms diagnostic timeout. SVD v3.23 records the corrected instruction-exact
unsigned-minimum rule; the value-five experiment remains evidence that raising
the scheduler did not cure the first-response loss.

The corrected open HIL then programmed scheduler 1 and packet PTI 1 for active
Probe Request, Authentication and Association. Active scan received the
directed Probe Response. Authentication read back configuration
`0x120003ff..0x120033ff` as contention backoff varied, with PTI vector fixed at
`0x00111110`. Five reset-to-connect repetitions all completed Association,
WPA2, protected ARP and IP setup; one received the first Authentication
response and four succeeded on attempt two after a full 1,000-ms first wait.
Thus SVD v3.23 is both instruction-exact and HIL-qualified, while PTI remains
excluded as the cause of the intermittent first-response loss.

Ordinary data has a separate instruction-exact event mapping. Complete
`libnet80211.a[ieee80211_output.o]::ieee80211_encap_esfbuf` maps IEEE user
priorities to the four EDCA queues and selects coexistence events 10, 11, 12
and 13 for Voice, Video, Best Effort and Background respectively. The complete
48-byte `libcoexist.a[coexist_core.o]::coex_pti_tab` maps those events to
packet PTI 3, 2, 1 and 1. Because each value is below event-one PTI 5,
complete `mac_tx_set_pti` retains the same value in the scheduler field.
`LegacyTxQueue::{vendor_data_packet_priority,
vendor_data_scheduler_priority}` now owns this finite mapping; upper data and
EAPOL paths no longer publish the unevidenced `0/0` pair.

An open PSRAM/PSRAM HIL run with this exact data mapping completed
Authentication, Association, WPA2, protected ARP and IP setup. It still
observed status-five unicast retries, and three connected 100-packet ICMP
series lost 31%, 25% and 33% with approximately 3-ms mean RTT. The mapping is
therefore retained as blob fidelity, not claimed as a repair for the remaining
ordinary-TX loss. Descriptor and queue-policy differences after PTI selection
remain the next comparison boundary.

Complete `libpp.a[hal_mac_tx.o]::mac_tx_get_rts_rate` closes a separate
rate-dependent gap in that standalone legacy path. Its exhaustive `0x96`-byte
body maps OFDM 48/24/54/36 Mbit/s to basic rate 24M, 12/18M to 12M, and 6/9M
to 6M. Complete `mac_tx_set_len` publishes that result in
`TX_Q_LENGTH_CONTROL` for every legacy PPDU, whether or not explicit RTS/CTS
protection is requested. The promoted migration had already reconstructed the
same selector in `tx_rate.rs`; the standalone HIL had incorrectly mirrored
the data rate into this field. `LegacyRate::vendor_rts_rate` now owns the
instruction-exact table, and the upper path selects the RTS power bytes
independently using the returned basic rate.

The controlled HIL rate sweep held the PSRAM/PSRAM image, channel 11 HT40-,
WPA2 peer, queue, power limit and packet interval fixed. At 6 and 24 Mbit/s,
both 200-packet samples had zero loss; 48 Mbit/s delivered 199/200. The
incorrect 54-Mbit/s image had repeatedly lost 25 to 39 percent. After restoring
the vendor 24-Mbit/s basic-rate field and its independent power selection,
54 Mbit/s delivered 197/200 and then 495/500, with 1.975-ms and 2.353-ms mean
RTT. Linux simultaneously decoded the station at exactly 54.0 Mbit/s. SVD
v3.24 records both the instruction-exact selector and this HIL qualification.
Rare ACK retries remain visible and are not claimed to be eliminated.

The remaining retry tail exposed a second, independent rate-control omission.
Complete `_oracles/libpp.a[trc.o]::rcGetRate` accumulates the attempt counts
from four `(rate, count)` pairs in the selected 12-byte schedule record; the
promoted migration `lmac.rs::select_basic_retry_rate` is instruction-equivalent.
For the 54-Mbit/s 802.11g record the exact ladder is `54M x2, 48M x2, 6M x3,
5.5M x25`. The standalone path instead repeated 54M for all four long-retry
attempts. `rate_schedule::schedule_rate_after_failures` and
`LegacyRate::vendor_retry_rate` now expose the existing Rust-owned schedule
through a safe value API. This finding describes TX policy rather than a new
MMIO field, so it belongs in source/provenance and does not manufacture an SVD
register.

The scan-to-associated receive-policy edge is split according to its physical
layout rather than grouped under a synthetic monolithic MAC peripheral:
`WIFI_MAC_BSSID_POLICY` begins at `0x2010_4004`,
`WIFI_MAC_INTERFACE_ADDRESS` at `0x2010_405c`, and
`WIFI_MAC_RX_FILTER` at `0x2010_40d8`. Complete
`libpp.a[hal_mac.o]::{hal_mac_rx_set_policy,hal_mac_set_rxq_policy}` bodies
prove every address, mask and fresh-read RMW. The complete
`libnet80211.a::wifi_set_rx_policy` parent identifies both relevant jump-table
leaves. Case five finishes with
`ic_set_rx_policy_ubssid_check(0, false)`, while case six finishes with
`ic_set_rx_policy_ubssid_check(0, true)`. A full vendor first-Authentication
capture on 2026-07-28 read queue-zero filter `0x0001c387`, exactly matching
case six. The open associated-STA transition therefore follows case six rather
than migration's earlier policy-five snapshot.

The policy bit meanings are recorded only to their supported confidence:
address-check and policy-enable follow the recovered branch behavior, while
the remaining mode/control/management/UBSSID positions retain `UNKNOWN`
suffixes where the hardware meaning is incomplete. Connected open STA
scan/authentication/WPA2/DHCP HIL qualifies queue zero; no claim is made that
all other queue/interface policy combinations have been exercised.

The policy correction alone reproduced the vendor filter word but did not cure
the open driver's intermittent Authentication failure, so it is not recorded
as that failure's root cause. A separate 2026-07-28 TX-power A/B localized the
remaining sensitivity more narrowly: the current open PHY/MAC profile at API
limit 80 quarter-dBm (ordinary queue power code 20) completed three consecutive
reset-to-DHCP runs, and a 200-packet host ping had zero loss with 4.613 ms mean
RTT. At API limit 20 quarter-dBm (queue code 5), repeated runs included complete
twelve-attempt Authentication failures. The full vendor driver succeeds with
queue code 5, so this evidence points to a remaining open/vendor power-table,
gain or calibration-path mismatch; it does not establish a direct code-to-dBm
interpretation.

Interface-address publication has independent instruction-exact evidence in
the complete pinned `libpp.a[hal_mac.o]::hal_mac_set_addr` leaf. It proves a
four-entry, `0x8`-byte stride and three ordered operations per entry: a full
low-address store, a full high-address store containing bytes 4..5, then a
fresh-read RMW setting bit 16. The earlier Rust cold path folded the last two
operations into one value-equivalent write; SVD v3.3 and the generated PAC now
preserve the complete leaf timing exactly.

The cold MAC handshake is a separate generated
`WIFI_MAC_COLD_HANDSHAKE` block at `0x2010_4de0`. Complete pinned
`libpp.a[hal_mac.o]::hal_init` offsets `0x00..0x3a` prove the REQUEST-bit RMW,
READY polling loop, then ordered `INT_ENABLE=0` and
`INT_CLEAR=0xffffffff` stores. The PAC keeps that successful hardware order
but replaces the blob's unbounded loop with a caller-bounded sample limit.

Queue-three RX policy at `0x2010_40e4` is also the promiscuous-sniffer control
word. Complete pinned `libpp.a[hal_sniffer.o]::hal_sniffer_enable` proves
seven ordered fresh-read RMW edges: set bit 17; clear bits 0, 1, 2, 3 and 8
separately; then clear bits 7 and 9 together. SVD v3.5 splits the formerly
combined bits 3:2 and high unknown range only as far as this evidence permits;
unresolved electrical meanings retain `UNKNOWN`.

Complete pinned `libpp.a[hal_crypto.o]::hal_crypto_init` independently proves
the five cold crypto stores at `0x2010_4800..0x2010_4810`: two
`0x00030000` interface images followed by three zero images. SVD v3.6 names
the otherwise unresolved fourth word at `0x2010_480c`
`INIT_AUX_UNKNOWN`; no algorithm meaning is inferred from its zero store.

Complete pinned `libpp.a[hal_mac.o]::mac_rxbuf_init` proves a four-RMW cold
RX-buffer prefix at `0x20104c68`, `0x20104c6c`, `0x20104c70` and
`0x2010407c`. SVD v3.7 records the two unresolved low-twenty-bit words and the
low-byte control as `UNKNOWN`, while the already qualified high-window field
retains its descriptor meaning.

The leaf's final store is intentionally outside this PAC prefix: it copies
the external `wDevCtrl` pointer into `RX_DESCRIPTOR_BASE`. In the open driver,
`rx::publish_cold_ring` owns that descriptor pointer and the corresponding
software-to-hardware lifetime edge.

Complete pinned `libpp.a[hal_mac.o]::hal_enable_mac` proves the cold MAC
enable transaction independently: one fresh-read RMW clears bits `7:4` at
`0x20104c00`, then a full store publishes the function's event-mask argument
at `INT_ENABLE` (`0x20104c40`). Complete `hal_disable_mac` sets the same four
gate bits. SVD v3.8 therefore names the group `MAC_DISABLE_GATES_UNKNOWN`
without guessing the individual gate meanings, and the generated PAC keeps
the gate edge and interrupt publication adjacent.

Complete rev0 ROM `phy_enable_low_rate` (`0x2f825210`, size `0x20`) and
`phy_disable_low_rate` (`0x2f825230`, size `0x20`) identify a separate PHY
baseband block at `0x20108060/0x2010807c`. Each leaf performs three fresh-read
RMW edges: bit 10 at `8060`, bit 11 at `8060`, then bit 11 at `807c`.

The earlier cold MAC transcription combined the first two disable edges into
one `0x0c00` mask. SVD v3.9 and the generated PAC restore the exact three-edge
order. MAC init now owns only the low-rate policy decision; it cannot see the
PHY register identities.

Complete pinned `libpp.a[hal_mac.o]::mac_last_rxbuf_init` (size `0xd2`)
publishes six three-word table entries at `0x20104124..0x20104170`. It then
sets `0x20104120` bits `13:8` and `6:1` through two separate fresh-read RMWs,
followed by a final bit-27 set at the multifunction RX/CSI word
`0x20104098`.

The former Rust transcription combined the two `0x4120` edges into one
`0x3f7e` RMW. SVD v3.10 records the six-entry geometry and unresolved fields,
and the PAC preserves all eighteen stores plus the three distinct enable
edges.

The direct prefix of complete pinned
`libpp.a[hal_mac.o]::mac_txrx_init`, offsets `0x08..0xd0`, contains eighteen
fresh-read RMW edges before its first external HE callback. SVD v3.11 records
that bounded prefix separately from `hal_he_set_mac_delay`,
`hal_he_set_ack_rate` and `hal_he_set_bbrxhung_time`.

This comparison found several earlier value-equivalent collapses: four
updates of `0x20104c8c`, separate queue-zero/one bit-24 and bit-26 edges,
separate bit-0 and bit-4 edges at `0x20104114`, and the bit-31 edge before the
field replacement at `0x20104118`. The PAC now preserves all eighteen in blob
order.

The complete direct suffix of the same function, offsets `0xee..0x16e`,
contains nine more fresh-read RMW edges after the three callbacks. SVD v3.12
records this as a separate generated-PAC transaction. In particular, bit 31
and bit 30 at `0x20104c1c` are distinct edges, as are the bits-30:16 group and
bit 31 at `0x20104c60`; the earlier Rust transcription combined each pair.
The suffix then clears RX walker-enable at `0x20104080`. Callback effects are
not claimed by either direct transaction.

Complete pinned `libpp.a[hal_mac_ctl.o]` bodies close that callback gap.
`hal_he_set_ack_rate(0)` performs four full-word stores at
`0x2010444c/0x20104458` and `0x20104450/0x2010445c`;
`hal_he_set_bbrxhung_time(0)` replaces the low twelve bits at
`0x20104c1c` with `0x00f`.

`hal_he_set_mac_delay` first calls `g_wifi_osi_funcs._env_is_chip` at table
offset `0x04`. The S31 esp-hal adapter returns true, so real hardware follows
the on-chip branch, calls `_random` at table offset `0x144`, and reduces its
result modulo eleven. The five resulting RMWs program `0x20104c58` three
times and `0x20104c54` twice. The former Rust fixed values followed the
function's FPGA branch and were therefore incorrect for the connected chip.

SVD v3.13 records the exact on-chip fields and cites the archive, pinned
esp-wifi-sys OS-adapter layout, and pinned esp-hal implementations. The MAC
crate owns the modulo-eleven value as `MacDelaySlot`; the integration supplies
entropy through a platform trait, so neither raw RNG MMIO nor the vendor C ABI
crosses into the open driver.

Complete pinned `libpp.a[hal_mac.o]::hal_init`, offsets `0x56..0x90`, applies
four direct fresh-read RMWs to each queue policy word before calling
`hal_mac_rx_set_policy(queue, 0, 0, 0)`. The complete leaf adds five more RMWs
for queues zero through two and rejects queue three. The cold transaction is
therefore 31 RMW edges, not the 13 combined updates in the former Rust
transcription.

The comparison also found a missing queue-one edge: after setting BSSID
policy bit 30, the leaf separately clears bit 31. SVD v3.14 splits the proven
filter bit 13 from the formerly opaque bits-16:11 group and records the exact
source without assigning an unproven electrical name. Generated PAC code now
owns the complete 62-operation read/write trace.

Complete pinned `libpp.a[hal_mac_tx.o]::hal_attenna_init` (the misspelling is
the archive's symbol name) contains two reverse traversals of eight words from
`0x20105510` down to `0x201051ac`, with a `0x7c` stride. The first traversal
clears bit 2 once per word. The second performs three distinct fresh-read
updates per word: clear bit 3, set bit 5, then clear bit 4. Two final RMWs at
`0x201042b0` clear bit 2 and set bit 5. This is 34 RMW edges in total.

The former Rust code combined each bank's four edges into one update. SVD
v3.15 records the evidenced fields without guessing electrical names, and the
generated PAC now preserves the complete order. The adjacent
`hal_mac_rate_autoack_init` symbol was also checked in full: its two-byte body
is only `ret`, so it has no omitted S31 MMIO effect.

The complete channel-change chain in `libnet80211.a[wl_chm.o]` and
`libpp.a[if_hwctrl.o,hal_mac.o,hal_pwr.o]` ends in
`pwr_hal_select_wifimac_regdma_link(4)`. A same-phase vendor/open pre-auth
comparison confirmed why this tail is not transient noise: vendor read
`0x2010d83c = 0x080801ff` (link four), while the open path which merely released
the MAC stop request read `0x080001ff` (link zero) and suffered connection-time
ACK failures. Restoring link four produced first-attempt Authentication,
Association, WPA2 and DHCP in the PSRAM/PSRAM profile. A 200-packet ICMP run
then had zero loss and 4.539 ms average RTT; open counters recorded 211 immediate
TX successes, three recovered ACK retries, no other failures and no hardware
timeouts. The obsolete link-preserving/link-zero restart variant was removed.
SVD v3.22 records this HIL qualification and the instruction-exact STA
TSF/wakeup fields described below.

The next bounded part of complete `hal_init`, offsets `0xcc..0x12a`, stores
interrupt mask `0x19a879e0`, repeats the bit-28 set at `0x20104c8c`, and
replaces the low two bytes at `0x20104098` through two separate RMWs. Complete
`hal_mac_set_rxbuf_reload_use_hw_beacon_enable` then sets bit 27 at
`0x20104080`.

`hal_init` obtains the following value from `g_wifi_osi_funcs` offset `0x148`.
The pinned S31 OS-adapter header and generated binding identify this slot as
`_slowclk_cal_get`. Complete `hal_timer_update_by_rtc(1, value)` sets bit 27
at `0x2010d830`, then replaces bits 17:0 at `0x2010d878` with the callback's
low eighteen bits. The pinned `esp32s31-async-platform` esp-hal adapter
currently returns zero for S31 with an explicit TODO; it does not access an
additional system peripheral.

SVD v3.16 records all seven tail operations. The integration exposes the
calibration through `MacSlowClockCalibrationSource`, so a future real clock
calibration remains platform-owned and does not reintroduce the vendor C ABI
or raw non-radio MMIO into the MAC crate.

Complete `hal_init` offsets `0x12e..0x190` finish with COEX/PTI setup. Together
with the complete setter leaves, this is seventeen distinct RMW edges at
`0x201042fc` and `0x20104dd4..0x20104ddc`; the former Rust implementation
combined the ordinary defaults into two edges and omitted all twelve OFDMA
and beamforming edges.

The complete `_oracles/libpp.a[hal_tsf.o]` leaves identify the shared STA TSF
control at `0x2010d858`. `hal_enable_sta_tsf` sets bits 31 and 27, then replaces
bits 22:19 with one; `hal_disable_sta_tsf` clears the same gates and mode.
`hal_set_sta_tsf_wakeup` controls bit 29 there together with bit 21 at
`0x2010d830`. Complete `_oracles/libpp.a[hal_pwr.o]`
`pwr_hal_set_mac_modem_state_wakeup_protect_enable/disable` independently sets
or clears bit 24 at `0x2010d858`. These meanings are instruction-exact rather
than names inferred from a register snapshot. A working vendor pre-auth
snapshot had wakeup protect disabled (`0x2010d858 = 0x88080000`), whereas the
standalone open initialization left it enabled (`0x89080000`) without owning
the matching vendor power-management lifecycle. A controlled open HIL run
which copied only the clear bit still produced q0 status 5 on all three auth
attempts; this bit is therefore documented hardware state, not a standalone
TX repair.

The callback at `g_wifi_osi_funcs` offset `0x1a8` is `_coex_pti_get`. Complete
`libcoexist.a::coex_core_pti_get` copies `coex_pti_tab[event]`; its complete
48-byte cold table gives event 1 = 5, event 3 = 7, event 10 = 3 and event 15 =
1. These are exactly the four events queried by the complete MAC tail.

There is an important disabled-feature oracle defect: the pinned esp-hal
no-COEX stub returns zero without writing its output pointer.
`hal_set_ofdma_sequence_pti` does not initialize its twelve stack output bytes
before reading them, so following that path would publish indeterminate PTI
values. SVD v3.17 records the intended table source and the complete setter
geometry. The MAC queries an explicit `MacCoexPtiSource` in exact blob order;
the current integration supplies the pinned cold-table values without linking
the vendor coexistence runtime.

Complete `hal_he_init` has a natural platform boundary at its
`hal_init_tx_pwr` child. SVD v3.18 records the complete prefix before that
call: the parent, complete `hal_init_bf`,
`hal_he_set_bf_report_rate(1, 0x10, 0, 0)`, and `hal_init_tb_tx`. The bounded
transaction contains twenty-two distinct RMW edges and a final volatile read at
`0x20107128`.

This full comparison corrected three earlier simplifications. The parent
clears bit 31 at `0x20104c80`, then clears bits 31:30 through a second RMW.
`hal_init_bf` replaces all eight low bits at `0x20104c78` (the previous mask
incorrectly preserved bit zero) and performs three omitted report-rate RMWs at
`0x20104464`. Finally, `hal_init_tb_tx` clears bits 15 and 14 at
`0x20104e04` separately. The remaining HE work begins with the PHY-dependent
TX-power table and is deliberately tracked as the next ownership boundary.

SVD v3.19 closes that power boundary. Complete `hal_init_tx_pwr` queries PHY
rates 0 through 42 in order, retaining both signed bytes returned for every
rate. Complete `hal_init_tb_power`, `hal_init_imrsp_power` and
`hal_init_tb_ru_power` then publish 56 separate RMW edges in
`0x20104408..0x20104448`. These six-bit values are PHY gain-table indices, not
dBm; treating them as radiated power would explain misleading power logs.

The old migration did not contain a second table builder. Its `lmac.rs` read
the already-built 86-byte `s_phy_get_max_pwr` table for legacy and HT frames.
The complete rev0 ROM producer is now recorded separately: each
`phy_get_max_pwr` call performs two ordered RMWs at `0x20107428`, derives two
quarter-unit targets from live PHY calibration and channel state, clamps each
to 84, then arithmetic-shifts by two. The MAC therefore owns table storage and
register publication, while a narrow PHY/platform source owns production of
each calibrated pair.

SVD v3.20 closes the remainder of `hal_he_init`. Before the register suffix,
complete `dbg_read_tx_power` queries rates 0 through 25 while skipping rate 4.
Its 25 results are discarded for logging, but the rev0 ROM producer still
causes fifty ordered PHY RMW edges; the open cold path therefore preserves the
queries through the same narrow source.

The following complete suffix is 163 writes/RMWs plus two guard reads. It
includes four separate ER-SU ACK-byte updates, the 120-word clear at
`0x201055f0..0x201057cc`, broadcast-RU and UORA fields, and the deliberately
repeated multi-BSSID updates in the parent and both children. Those operations
now execute through generated fields under the unique `RadioRegisters`
borrow. The former upper MAC implementation combined several RMWs, omitted
broadcast-RU/UORA and represented the register set as raw numeric aliases; it
and its now-unused aliases have been removed.

SVD v3.21 moves the final open promiscuous receive boundary out of generic MAC
MMIO. Complete `hal_sniffer_enable` remains the seven instruction-exact RMWs
at queue-three policy word `0x201040e4`. The surrounding zero image at
`0x20104cac` and bits 15:8 update at `0x201040f4` are explicitly attributed to
the open-driver frontier first recorded in commit `6a2b358` and qualified by
the connected scan/STA HIL; they are not mislabeled as vendor behavior.
Complete `hal_sniffer_set_promis_misc_pkt` independently proves the latter
register but has additional mode-dependent branches that the open path does
not claim. The PAC now owns the complete store/RMW/fence sequence, leaving the
upper cold-init function with semantic capability calls only.

SVD v3.26 starts a systematic audit of the pinned debug decoders. The archive
exports 43 complete `dbg_*` functions from `hal_debug.o`; these are unusually
strong sources because their instruction-level masks are paired with the
blob's own field-name format strings. They are not all register decoders:
`dbg_dump_trig_*`, `dbg_dump_rx_ppdu`, `dbg_dump_tx_link` and related helpers
primarily decode packet, descriptor or in-memory structures. The highest-value
remaining MMIO decoders are:

- `dbg_read_ax_diag` and `dbg_read_axtb_diag` for received Trigger and
  trigger-based TX results;
- `dbg_read_bsr_info`, `dbg_read_muedca_timer` and
  `dbg_read_txq_conf1/2` for BSR, TID, MU-EDCA and per-queue TB policy;
- `dbg_read_rx_misc`, `dbg_read_color_collision` and `dbg_read_rx_count` for
  HE BSS matching, beamforming, color collision and RX failure counters;
- `dbg_read_tx_sig`, `dbg_read_tx_ppdu`, `dbg_read_tx_mplen`,
  `dbg_read_ack_rate`, `dbg_read_cts_rate` and `dbg_read_bfr_rate` for TX
  vector and response-rate cross-checks;
- `dbg_read_internal_txba`, `dbg_read_rx_ba` and `dbg_read_key_entry` for
  BlockAck and crypto-table geometry.

This version fully decodes `dbg_read_bsr_info`, including the eight
interleaved WDEVTXQBSR hardware/software values, validity bits, TID bitmap and
the named WDEVAXOPTIONS fields. It also decodes all ten `WDEVAXDIAG` words at
`0x20104508..0x2010452c`: matched AID, RU, UL MCS/DCM/FEC, GI/LTF, bandwidth,
spatial reuse and Trigger common fields are now generated PAC fields. The
blob explicitly warns that a following RX frame can overwrite this diagnostic
block, so Rust exposes it as a best-effort snapshot rather than protocol
state.

Complete `dbg_tb_ignore_cca_enable` proves the polarity of
`0x20104c7c[12]`: one means trigger-based TX respects CCA and NAV, zero enables
the debug ignore mode. Complete `hal_he_disable_obss_narrow_bw_ru` adds the
two-word `0x20104e9c..0x20104ea0` boundary. Its aggregate enable/disable
meaning and update order are exact, while the individual twenty-bit bitmap
and five-bit class identities remain explicitly approximate.

The same audit exposed one missing runtime edge. Complete HE AddBA response
and DELBA paths call `hal_he_set_tid_bitmap`/`hal_he_clr_tid_bitmap`; the open
software BlockAck session had not mirrored this hardware-visible BSR state.
The connected HE HIL now performs the update only at the owned
`Operational`/DELBA transitions. After successful TID0 AddBA with window 32,
two snapshots read `TID_BITMAP=0x01`; hardware/software BSR values remained
zero because the controlled AX211 AP emitted no Trigger frames. Twenty-four
complete DCM A-MPDU matrix rounds remained clean. This qualifies the lifecycle
write, but not OFDMA scheduling.

SVD v3.27 continues with `dbg_read_txq_conf1`, `dbg_read_txq_conf2` and
`dbg_read_muedca_timer`. The first decoder names TID, TB enable, MU-EDCA timer
selection, MPDU-length link address and minimum TX power in all eight
reverse-addressed queue words. The second resolves the formerly historical
`HT_SPACING_*` names: the three ten-bit lanes are `TXQ_MIN_MPDU_LEN` for
CBW20, CBW40 and CBW80, followed by software RTS and software CTS. This agrees
with the existing HE delimiter HIL: the same rate/density-derived minimum
subframe length must be copied into each channel-width lane.

The four MU-EDCA words at `0x20104dbc..0x20104dc8` now expose timer and current
count in eight-TU units, enable, reset, reached and AIFS. A fixed-allocation
PAC snapshot normalizes reverse physical queue addressing back to the logical
queue numbers logged by the blob.

`HIL_OPEN_HE_QUEUE_SCHEDULING_BASELINE_2026_07_29` exercised that snapshot
after HE association, WPA2 Message 3, successful TID0 AddBA with window 32,
and ten clean three-profile DCM A-MPDU matrix rounds. All eight queue-one
`TB_ENA` fields were clear and their TID, MU-EDCA selection, MPDU-length-link
and minimum-power fields were zero. All four MU-EDCA timers were disabled with
zero timer/count/AIFS and `REACHED=1`. Queue-two CONF2 alone contained
`TXQ_MIN_MPDU_LEN=(3,3,3)`; the other three queues contained zero and all
software RTS/CTS bits were clear. Trigger reception, TB transmission and
TB QoS Null counters remained zero. This qualifies register geometry, logical
queue ordering and the ordinary HE-SU idle baseline; it does not qualify an
OFDMA scheduling transition because the AX211 AP emitted no Trigger frame.

SVD v3.28 decodes two more complete `hal_debug.o` bodies.
`dbg_read_color_collision` proves the low/high HE BSS-color bitmap words at
`0x20104040/44` and names the overlapping bitmap/clear control, timeout and
collision-threshold fields in the already-canonical `0x20104048` HE-init
register. The SVD deliberately does not create a second alias for that control
word.

Complete `dbg_read_rx_count` proves 39 named statistics across
`0x2010430c..0x20104398`, `0x20104c64` and `0x20104e18..0x20104e1c`.
The generated PAC preserves its ten-bit and sixteen-bit masks. In particular,
the blob does not read CFO from a separate register: it sign-extends the upper
half of the `WDEVRX_MPDU` word and multiplies it by 40. The typed snapshot
performs that same finite transform and groups ordinary RX, decoder-error and
hang/panic counters without exposing raw addresses.

`HIL_OPEN_RX_STATISTICS_BASELINE_2026_07_29` then qualified the read path after
HE association, WPA2, TID0 AddBA and one complete 30-profile matrix round.
The coherent snapshot contained MPDU 6216, end 6197, data-success 286, FCS 42,
power-drop 42, HT-SIG error 8, HE-SIG-A CRC 1, ACK interrupt 25 and RTS
interrupt 6. FIFO overflow, buffer full, TKIP, abort and all hang/panic
counters were zero. The color bitmap contained bit 25 with threshold one and
timeout 60 seconds. Four following 30-profile HE rounds remained clean. These
are cumulative diagnostic values, not packet-loss ratios; later performance
experiments must use before/after deltas around the measured workload.

SVD v3.29 completes the field-level `dbg_read_rx_misc` audit. It names all
twenty low RX-filter policy bits, the interface BSSID AID/MMSS/SoftAP fields,
the HE BSS-color and multi-BSSID controls, WDEVAXOPTIONS1/2, five nominal
packet-padding lanes, RX power-save timing, three custom receive-type/ACK
profiles, BSSID-list position and beamforming timing/control. Duplicate
descriptions of `0x20104c80` were removed in favor of one canonical
WDEVAXOPTIONS2 register.

The same debug-symbol sweep found two previously under-described areas.
Complete `dbg_read_wdevdelay` names `0x20104c54[20:10]` as
`WDEV_TXCCK_DELAY` and `0x20104c58[20:10]` as `WDEV_RXCCK_DELAY`. Complete
`dbg_read_ack_rate` and `dbg_read_cts_rate` prove that the response-rate area
is six consecutive words, not four anonymous images:
`ACK_TAB0`, `ACK_TAB1`, `ACK_SCCKTAB`, `CTS_TAB0`, `CTS_TAB1` and
`CTS_SCCKTAB` at `0x2010444c..0x20104460`. Their OFDM, CCK and SCCK byte lanes
now have generated field names while the cold setter retains its original
four full-word writes.

The PAC exposes one allocation-free `he_receive_configuration_snapshot` for
the recovered HE BSSID, UL-MU disable, NFRP, TRS, packet-padding, RX
power-save, custom-type and beamforming fields. The complete inventory and
MMIO-versus-descriptor classification of the remaining debug functions is in
`docs/esp32s31-debug-oracles.md`.

`HIL_OPEN_HE_RX_CONFIGURATION_BASELINE_2026_07_29` exercised that snapshot
after HE association, WPA2, TID0 AddBA and a clean 30-profile matrix round.
The active ordinary-AP image had MPDU-length offset 380, NFRP threshold 8,
hardware TXOP, BSR update, TB stop, TRS, UL-MU and UL-MU data enabled, and HE
response ACK enabled. HE beamforming was enabled with BFRP time 16, NDP time
113 and hardware-sequence selector 5. RX power-save itself and all three
custom receive types were disabled. Trigger/TB and all hang/panic counters
remained zero. This qualifies the read path and idle configuration, not an
actual OFDMA or beamforming exchange.

SVD v3.30 decodes five further complete `hal_debug.o` bodies.
`dbg_read_rx_ba` is misleadingly named: its own strings and address walk prove
eight reverse-addressed `WDEVTXQBA` TX BlockAck result banks. Logical queue
zero occupies `0x20105528..0x20105538`; each following queue descends by
`0x7c`, ending with queue seven at `0x201051c4..0x201051d4`. Every bank
contains bitmap high/low, starting sequence plus TID/control, and transmitter
address high/low. The existing three-read TX completion API remains unchanged
so diagnostic address reads do not add latency to the hot path; a separate
five-word snapshot exposes the complete debug evidence.

Complete `dbg_dump_txq_txinfo` adds one adjacent word per queue at
`0x20105524-q*0x7c` and resolves upper bits in the TX-BA transmitter-address
high word. The resulting diagnostic snapshot reports ACK, BlockAck, ACK TID,
whether the last transmission was trigger-based and the seven-bit
trigger-based packet count. These are directly useful for distinguishing a
received Trigger frame from a subsequently scheduled TB transmission.

Complete `dbg_read_internal_txba` independently proves the standalone
`0x2010429c..0x201042ac` TX BlockAck result: 64-bit bitmap, two raw
transmitter-address words, fragment number, starting sequence and TID. The
blob does not identify byte ordering inside the raw address pair, so the PAC
does not invent it.

Complete `dbg_read_key_entry` names queue PLCP1 bits 22:17 as the six-bit key
entry index and bits 24:23 as the BSSID selector. It then indexes the already
owned ten-word-per-entry key table at `0x20105800` with stride `0x28` and
checks the corresponding validity bit at `0x20104814`. Finally,
`dbg_read_tx_mplen` proves that the 120-word aperture cleared by
`hal_he_init` at `0x201055f0..0x201057cc` is the TX MPDU-length link table:
bits 13:0 are `MPDU_LEN`, bits 20:14 are `NEXT`, and bits 31:21 remain
unnamed. Generated fields and an allocation-free typed reader now own this
map.

SVD v3.31 extends the audit beyond `hal_debug.o` into complete
`libpp.a[pp_debug.o]` bodies. `dbg_lmac_hw_statis_dump` independently
cross-checks the existing RX statistics aperture and adds `cts_int` at
`0x20104384`, `trigger` at `0x2010439c`, plus `txrts`, `txcts`, `track` and
`trcts` at `0x20104e08..0x20104e14`. It also reads the already canonical
combined hang/panic words at `0x20104e18/1c` under the shorter
`txhung`/`panic` labels. Because this function loads complete words without
masks, the SVD records exact address/name associations but does not invent
counter widths or expand the abbreviated `trcts` label.

Complete `dbg_lmac_diag_statis_dump` establishes the sparse diagnostic map:
`diag4..diag7` at `0x201043b4..c0`, `diag8..diag10` and `diag12` at
`0x201044e0..ec`, and `diag0..diag3` plus `diagsel` at
`0x20104e50..64`. The missing `diag11` is absent from the complete body and is
therefore not synthesized. These words are mapped as opaque observations
until a selector experiment or another complete hardware leaf proves their
higher-level meanings.

SVD v3.32 follows that rule with the actual write-side Trigger path rather
than another passive decoder. Complete blob
`hal_mac_tx.o::{mac_tx_set_hesig,mac_tx_set_tb,mac_tx_set_mplen}` and the
independent rev0 ROM bodies agree on this chain:

1. `mac_tx_set_hesig` enters `mac_tx_set_tb` for an HE frame unless
   descriptor-state word `+0x30` bit 16 suppresses it.
2. The QoS TID must belong to the configured bitmap. Complete
   `if_hecfg.o::wifi_he_get_hetb_tid_bitmap` maps limits one through four to
   `[0x01, 0x81, 0xa1, 0xa3]`; the normal/fallback limit three therefore
   admits TIDs 0, 5 and 7.
3. `mac_tx_set_mplen` allocates and writes a `MPDU_LEN/NEXT` chain in the
   already mapped 120-word aperture. It publishes the terminal seven-bit link
   at `0x201054fc-q*0x7c`, a previously unnamed vector word.
4. `mac_tx_set_tb` sets `0x20104dbc+4*q[8]`, constructs
   `WDEVTXQ_CONF1` at `0x20104d68-0x10*q` from preserved bit two plus
   `TB_ENA`, queue, first link and TID, updates the corresponding low-twenty
   `WDEVTXQBSR_SW` value at `0x20104d78+8*q`, then publishes
   `1<<q` into `WDEVTXQBSR_CTRL[7:0]`.

The exact non-single-TID link ranges partition all 120 entries: q0 32..63, q1
64..95, q2 0..31 and q3 96..119. The typed PAC consequently replaces the
blob's mutable allocation bitmaps with `MacHeTbLinkReservation`: one live
aggregate per hardware queue receives a deterministic, disjoint contiguous
range. `prepare_he_trigger_based_queue` validates TID, entry count, all
fourteen-bit MPDU lengths and the twenty-bit BSR value before MMIO, preserves
the instruction order, and uses the validity RMW as its final publication
edge. `hal_mac_tx_clr_mplen` independently proves that hardware-visible
cleanup clears `TB_ENA`; its remaining work only frees the blob's software
bitmap.

This closes the missing internal write-side transition. It does not claim
UL-OFDMA HIL: a real AP Trigger and observed nonzero Trigger/TB counters are
still required.

SVD v3.33 closes the remaining direct-MMIO gap in the complete
`hal_debug.o::dbg_read_tx_sig` decoder. Besides independently checking the
existing HE-SIG-A words, the function decodes:

- the low HT MCS nibble and sixteen-bit HT length;
- internal VHT signal words at `0x201054ec/4f0-q*0x7c`, including MCS,
  channel width, group ID, NSTS, SU partial AID, SGI, coding and the
  SU-CBW20 pre-EOF-padding length;
- the HT40 length, MCS, EOF and TXOP counts at
  `0x2010550c-q*0x7c`;
- the RTS rate, HT20 EOF/TXOP counts and two three-bit antenna-selector
  groups at `0x20105510-q*0x7c`.

These are instruction-exact internal vector fields. Mapping the VHT branch
does not claim that the public ESP32-S31 station feature set supports VHT.

The same exhaustive sweep bounded every exported Wi-Fi `dbg_*` symbol: 69
unique functions across `libpp.a` and `libnet80211.a`. The remaining symbols
are packet/descriptor decoders, software-counter dumps, allocator checks or
empty debug stubs and add no fixed MMIO addresses. The rev0 ROM ELF has no
equivalent callable `dbg_*` decoder suite.

Two reachable nontrivial leaves add useful hardware evidence. The
`dbg_clr_hw_count` target `esp_test_clr_hw_statistics` asserts
`0x20104c00[16]`, pulses `0x20104308[0]`, then clears the arm bit. The
beamforming test helper `esp_test_get_bfr_avgsnr` reads
`0x20105f94[7:0]` and computes stream-zero average SNR as
`(code + 88) / 4` dB, treating `0x7f` as a saturated lower bound. The latter
is now exposed as an integer quarter-dB PAC snapshot for future BFRP/SU
beamforming HIL.

`HIL_OPEN_TX_BLOCK_ACK_DIAGNOSTIC_2026_07_29` qualified the v3.30 TX result
map under a real HE20 MCS0..9 A-MPDU matrix. After TID0 AddBA with window 32,
logical queue two reported BlockAck, bitmap `0x00000000ffffffff`, and
transmitter address `70:15:fb:a8:48:f0`; the standalone TXBA result reported
the same bitmap and address words. The queue result simultaneously reported
`LAST_TX_IS_TB=0` and `TB_PACK_SENT=0`, while both decoded HE Trigger RX and
TB TX counters remained zero. Thus the capture separates a working HE-SU
A-MPDU/BlockAck path from the still-unobserved UL-OFDMA transition. The blob's
four-bit `ACK_TID` field read as eight even though the negotiated and
standalone TXBA TID was zero; its source name and bit geometry remain exact,
but software must not yet treat that diagnostic value as the negotiated TID.

`HIL_OPEN_HE_FRITZ_TRIGGER_BASELINE_2026_07_29` repeated the complete
MCS0..MCS9 by three-GI HE-SU A-MPDU matrix against a nearby FRITZ!Box 7530 on
channel 6. Authentication, association, WPA2, protected RX/TX, DHCP and TID0
AddBA completed on the first attempt. Queue two continuously reported
BlockAck and up to 32 acknowledged subframes; MCS9/0.8-us-GI reached
79.9 Mbit/s of generated Ethernet payload. The peer advertised triggered SU
feedback and triggered CQI, but more than seven complete matrix rounds still
left the decoded frame counter, `RX_TRIGGER`, `TB_TX`, `LAST_TX_IS_TB` and
`TB_PACK_SENT` at zero. Capability advertisement is therefore not evidence
that an AP scheduler will issue an uplink Trigger.

`HIL_OPEN_HE_ANDROID_SOFTAP_TRIGGER_BASELINE_2026_07_29` used a OnePlus
CPH2653 running Android 16 as a second independent AP. Android `dumpsys wifi`
reported `config_wifiSoftapIeee80211axSupported=true`,
`config_wifiSoftapHeSuBeamformerSupported=true`,
`config_wifiSoftapHeSuBeamformeeSupported=true`, and
`config_wifiSoftapHeMuBeamformerSupported=false`; the user-visible Wi-Fi 6
switch was enabled for the 2.4-GHz SoftAP. The open station completed WPA2,
hardware CCMP RX/TX, ARP, DHCP, TID0 AddBA and repeated clean 30-profile
HE-SU matrices after the HIL used the SoftAP's actual `10.123.67.90/24`
subnet. The association response exposed BPSK DCM but no triggered SU/MU
feedback or triggered CQI. Trigger reception and TB transmission remained
zero and every queue retained `TB_ENA=0`. This device qualifies HE-SU and
CCMP interoperability, but its own capability/configuration evidence makes it
unsuitable as a UL-OFDMA or MU-beamforming oracle.

SVD v3.34 makes the recovered map structurally checkable instead of relying
only on the permissive local parser. All 46 register arrays now place the
CMSIS-SVD dimension group before `name`; 45 required correction and
`PHY_I2C_COMMAND_RAM.COMMAND_MEMORY%s` was already canonical. The resulting
file passes Arm's official `CMSIS-SVD.xsd`. `pac-gen` now rejects:

- a non-canonical dimension group or overlapping fields;
- any new absolute-address alias outside the explicit 28-address audit set;
- an undefined or duplicate provenance source;
- an invalid or overlapping address window.

The generator reads its MMIO windows directly from
`openEspRadioAddressWindows`, eliminating the former duplicated Rust
constant. The existing 28 register-address aliases remain deliberately
allowlisted for a separate canonical-peripheral/alternate-register change,
because that work can alter the generated public PAC API.

The only field overlap was resolved without losing information.
`hal_debug.o::dbg_read_color_collision` prints `RX_HE_BSS_COLOR_CONF[1:0]`
as `BITMAP` and separately prints its low bit as `COLOR_BITMAP_CLR`.
Consequently the SVD retains one two-bit `BITMAP_CONTROL` field, while the
typed PAC snapshot derives `color_bitmap_clear` from bit zero.

This revision also closes every previously dangling `SOURCE[...]` reference
and records the complete `hal_debug.o::dbg_read_tx_ppdu` body. That decoder
names frame-state halfword `+0x22` as `msdu_len`; complete blob and ROM
`mac_tx_set_tb` sum exactly those halfwords into `WDEVTXQBSR_SW`. The
Trigger-based owner must therefore retain original MSDU lengths separately
from its larger encoded MPDU/PSDU lengths.

SVD v3.35 removes the temporary address-alias allowlist. The 28 duplicate
physical addresses were audited against their cited complete ROM/blob bodies;
none required a mode-dependent register identity. Each was a partial view of
one multifunction word, so its independently proven fields now live under one
canonical register. This includes the PBus/front-end word, BSSID/HE station
word, HE color control, queue/protection/vector banks, completion diagnostics
and antenna/vector fields. The resulting recovered map has no duplicate
physical register address. A future genuinely mode-dependent view must be in
the same peripheral and register scope and must declare `alternateRegister` or
`alternateGroup`; creating another peripheral singleton at the same address is
rejected.

The generator now expands peripheral, cluster, register and field arrays
before checking physical identities. It rejects unmarked or access-inconsistent
alternate views, overlapping register byte ranges, duplicate expanded names,
misalignment, short array strides, out-of-range or overlapping fields, filler
`PRESERVED*` fields, undefined `SOURCE` identifiers and values outside the
five-value `CONFIDENCE` vocabulary. Eighty-eight filler fields and the four
remaining `PRESERVED_[0-3]` power-word gaps were removed; `svd2rust` RMW
writers preserve those omitted bits. Instruction-proven `*_UNKNOWN` fields
remain because vendor code actually changes them.

Access effects are encoded only where the cited instructions or HIL prove
them. `WIFI_MAC_INTERRUPT.CLEAR` is W1C,
`RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` is a hardware-cleared append doorbell,
and mixed control/status registers expose proven ready/comparator fields as
read-only. Software-generated set/delay/clear and commit/latch edges remain
ordinary RW fields because the sources explicitly write both edges; they are
not falsely described as hardware-self-clearing. There are no inferred
`resetValue` entries. Reset images remain deferred until an HIL snapshot names
the silicon revision and power/reset domain.

## SVD v3.36: Trigger-queue publication boundary

The owned HE A-MPDU path now retains each original MSDU length separately
from its MPDU and PSDU lengths and can publish the complete
`mac_tx_set_tb` transaction without a vendor scheduler. On ESP32-S31 rev0,
the standard PSRAM/PSRAM image completed HE20 association, WPA2/CCMP and a
TID0 AddBA window of 32. Before the HE queue doorbell, typed PAC readback
confirmed logical queue two, TID0, `TB_ENA`, MU-EDCA selector/timer, the
contiguous MPDU-length chain 0 through 31, terminal link 31 and validity bit
two. The following ordinary MCS9 A-MPDU received a full 32-bit BlockAck.

One boundary remains deliberately unresolved. The exact ROM sequence was
given a 47,104-byte MSDU sum, but `WDEVTXQBSR_SW[2]` read zero immediately
after the validity RMW. Removing the extra Rust device fence and reproducing
ROM's exact `SW(BSR)`, `LW(CONTROL)`, `SW(CONTROL)` order did not change the
readback. Since the AP emitted no Trigger and both RX-Trigger and TB-TX
counters remained zero, this HIL cannot distinguish a live hardware-consumed
value from a write-side latch that is not reflected by the read path. The SVD
records the observation but does not change access semantics or invent a
reset value.

## Cross-chip comparison

Current public ESP-IDF headers for ESP32-C5 and ESP32-C61 independently use the
same MODEM_SYSCON/LPCON naming model and R/W clock fields. This is useful for
detecting transcription errors, but it is deliberately secondary evidence:
only S31-local headers/SVD or complete S31 instruction bodies can create an
S31 PAC field.

Primary public comparison sources:

- Espressif ESP-IDF
  [ESP32-S31 PMU register header](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32s31/register/soc/pmu_reg.h);
- Espressif ESP-IDF
  [ESP32-C5 MODEM_SYSCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c5/include/modem/modem_syscon_reg.h)
  and
  [MODEM_LPCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c5/include/modem/modem_lpcon_reg.h);
- Espressif ESP-IDF
  [ESP32-C61 MODEM_SYSCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c61/include/modem/modem_syscon_reg.h)
  and
  [MODEM_LPCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c61/include/modem/modem_lpcon_reg.h);
- the public
  [esp-rs ESP32-S31 PAC/SVD package](https://github.com/esp-rs/esp-pacs/tree/main/esp32s31).

## Remaining uncertainties

The individual identities inside the RX/TX two-bit clock pairs, the electrical
meaning of the four force-TX/RX encodings, the PBus packed result windows, and
the two clock-selection subfields remain unknown. They are included with
`UNKNOWN` names because their locations and complete operations are useful
and instruction-exact.

Other raw PHY MMIO constants still follow the same migration rule: move an
address into the SVD only when its identity is stable; add named fields only
when S31-local layout or instruction-level masks support them.
