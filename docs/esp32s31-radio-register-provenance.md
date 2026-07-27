# ESP32-S31 radio register provenance

This register pass separates three questions that were previously mixed in
raw constants:

1. Which MMIO address is accessed?
2. Which bit field exists on ESP32-S31?
3. Which initialization order and value has actually been exercised?

The SVD answers the first two. HAL operations carry the third in comments and
through `PowerOperation::evidence()`.

## Clock and power chain

| Stage | Registers | Layout evidence | Operation evidence |
|---|---|---|---|
| HP-active modem ICG selection | `PMU.HP_ACTIVE_ICG_MODEM`, `IMM_MODEM_ICG`, `IMM_SLEEP_SYSCLK` | S31 SVD and PMU headers | pinned S31 `esp-hal` clock initialization |
| Modem bus and source | `HP_SYS_CLKRST.MODEM_CTRL0`, `MODEM_CONF` | S31 SVD | pinned S31 `esp-hal` clock initialization |
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
SVD has no write-trigger access class. This recovered SVD therefore models
the register `read-write`, records the conflict in its description, and
preserves the blob's read/modify/write sequence in the HAL.

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
baseband update at `0x20109c18` is represented on the already exact S31
`MODEM_SYSCON.WIFI_BB_CFG` register, alongside the unrelated PBus settle
condition.

The `phy_agc` HAL preserves the complete ROM access order: fifteen ordered
baseband-update writes, three fresh-read edges when enabling AGC, one
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
3:2 of shared `PHY_I2C_MASTER.MASTER_CONTROL` at `0xf818`. That register
already owns the independently proven master-register enable and mode fields,
so no second alias is introduced.

The `phy_agc` and `phy_i2c` HAL methods preserve the two DC-memory reads and
the single BBPLL RMW. Cold initialization, register initialization, and
channel changes all pass their existing `RadioRegisters` borrow. The raw C
ABIs, address constants, and duplicate mask helpers are deleted; the
source-only audit rejects raw `0x703c` and `0xf818`.

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
- Wi-Fi enable, BSS-CBW, MAC-baseband and cold-start fields on the already
  exact `MODEM_SYSCON.WIFI_BB_CFG` identity;
- TX-cap publication through existing
  `PHY_I2C_COMMAND_RAM.COMMAND_MEMORY[1]`, whose block/register/data byte
  layout already describes `value << 16 | 0x026b`.

The `phy_frequency` HAL preserves all fresh-read edges, full-word constants,
packed images and ROM branch encodings. Host tests record both reads and
writes, not only final values. Cold init, baseband init, D-code and every
channel action now borrow the same `&mut RadioRegisters`; the D-code MMIO
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

The pair-level functions do not distinguish the two constituent clocks, so
the SVD deliberately calls them `RX_CLOCK_ENABLE_PAIR` and
`TX_CLOCK_ENABLE_PAIR`. The four force encodings are exact, but their
electrical meaning is still unknown and remains
`FORCE_TXRX_MODE_UNKNOWN`.

The complete `phy_pbus_rd` address and shift tables expose five packed result
words at `0x20100894..0x201008a4`. Each visible nine-bit window is represented
without assigning an analog meaning. Only selector 1's low window has a
qualified consumer identity: the RX-DCO calibration path.

The PHY-I2C PAC records both host command words, the master control fields,
read-mask callback, opaque 14-bit host map, three clock-selection words, and
all 45 command-memory entries. Command layout is instruction-exact:
block/register/data occupy bytes 0/1/2, followed by write, busy and
start/reset bits 24/25/26.

The corresponding HAL is split by hardware ownership:

- `analog_i2c` owns PMU power/reset sequencing;
- `pbus` owns command publication, completion sampling, packed reads and the
  RX/TX clock pairs;
- `phy_i2c` owns host commands, the six-write clock-selection transform,
  master setup and bounded command RAM.

Every public operation documents both its S31 register-layout source and the
complete ROM/blob body used for operation order. The cold PHY binding accepts
`&mut RadioRegisters` borrowed from `Radio<P, Powered>` and uses these HAL
methods for the newly recovered regions. Reusable RFPLL, RXIQ/TXIQ, DCO,
gain, temperature, saturation, power and power-detector target bindings use
the same borrow; no raw-owner PHY-I2C or PBus force-test leaf remains.

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
- `PHY_I2C_MASTER.HOST_COMMAND_0/1` own the reset command at bit 26 and the
  sampled busy state at bit 25;
- `MODEM_LPCON.TICK_CONF.MODEM_PWR_TICK_TARGET` owns the six-bit fixed-crystal
  tick target.

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
`0x2010_f800`, and `0x2010_f804` literals in the live PHY crate. The shared
`0x2010_0890` word still has separately evidenced RXIQ status consumers on the
remaining raw frontier, so the audit rejects the deleted force wrapper rather
than falsely banning that physical identity.

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
