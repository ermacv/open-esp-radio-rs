# Line audit: `libphy.a[phy_init.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines nineteen external code functions, the four-byte
`g_phyFuns` pointer cell, and the 508-byte `phy_param` image. Every instruction,
relocation and branch of all nineteen functions was inspected. Sixteen are
strictly closed: seven **NO-REGISTER-EFFECT**, six **NOT-PORTED**, and three
**MISMATCH**. Three direct bodies are **BODY-AUDITED** pending strict closure
of their register-relevant ROM children.

## `phy_get_xtal_freq`

Size `0x40`, IRAM section offset zero. Strict status: **MISMATCH**.

The function calls `rtc_clk_xtal_freq_get`. A result of 32 stores parameter
code 2 at `phy_param[0x4f]`, 26 stores 1, and every other result stores zero.
It then fresh-reads `0x2010f028`, replaces bits 5:0 with
`(frequency_mhz - 1) & 0x3f`, and writes the result.

Rust has the same parameter-code transform, but the active register prelude
fixes the complete operation to 40 MHz and cannot reproduce the vendor's 26,
32, or other returned-frequency register images. This is a profile/domain
mismatch, not a vendor defect.

## `phy_reg_update_new`

Size `0x70`, IRAM section offset `0x40`. Strict status:
**BODY-AUDITED**.

The exact order is:

1. fresh-read `0x2010705c`, set bit 26, write it;
2. call `phy_wifi_agc_sat_gain(0x081812d)`;
3. fresh-read `0x20107104`, replace bits 8:0 with `0x1c0`, write it;
4. fresh-read `0x201078c8`, replace bits 6:0 with `0x17`, write it;
5. fresh-read `0x201078c8` again, replace bits 13:7 with `0x17`, write it;
6. tail-call `phy_set_ftm_en(1)`.

The reached Rust baseband/register owner contains these fixed updates and its
input-one FTM leaf matches. Strict promotion waits for closure of ROM
`phy_wifi_agc_sat_gain`.

## `phy_wakeup_init`

Size `0x188`, IRAM section offset `0xb0`. Strict status: **NOT-PORTED**.

The wakeup root first replaces bits 1:0 of `0x20100028` with 2, forces TX/RX
off, disables hardware frequency control, resets the I2C master, opens FE/BB
clocks, enables BBPLL calibration, bias and temperature-sensor power, obtains
XTAL frequency, and calls `phy_open_i2c_xpd_new(0)`.

It then calls, in order, PBus clear, I2C clock select 8, FE TX/RX reset, ADC
rate 1, I2C-master register init, frequency-register init `(2,4)`, FE init,
FE update, PWDET init, the absent parallel `phy_i2c_init2`,
`phy_freq_i2c_data_write(0)`, channel-frequency restore from
`phy_param[0x11c]`, PBus-register restore, PHY register init, this member's
register update, BB/AGC update, channel-register mode 1, TX-cap restore,
CBW/channel restore, AGC enable, frequency-ready wait, clock-generator reset,
hardware-frequency enable, BBPLL calibration disable, and TX/RX release.

It clears `phy_param[0x17]` and `[0x195]`, clears bits 1:0 of `0x20100028`,
and conditionally disables Wi-Fi when `phy_param[0x196]` is nonzero. Rust has
no wakeup lifecycle owner; the cold-start transition is not an equivalent
replacement.

## `phy_xpd_rf_new`

Size `0x62`, IRAM section offset `0x238`. Strict status: **NOT-PORTED**.

The body disables AGC, clears bits 1:0 of `0x20109c18`, delays 1 microsecond,
writes value 7 to block `0x67`, host 1, register 2, then:

- reads `0x20704184`, retains its low 16 bits, and full-writes that value back;
- reads `0x207040f0`, clears bits 27:0, and writes it;
- tail-calls `phy_close_fe_bb_clk`.

No Rust shutdown owner performs this sequence.

## `phy_close_rf`

Size `0x96`, IRAM section offset `0x29a`. Strict status: **NOT-PORTED**.

If `phy_param[0x195]` is zero it samples temperature first. If the registered
flag at `phy_param[0x25]` is zero, it then returns without radio writes.
Otherwise it calls the no-op I2C critical hook, disables hardware frequency,
forces TX/RX off, calls `phy_xpd_rf_new`, enables BBPLL calibration, writes
`0x77` to block `0x6a`, host 1, registers 0 and 1, stores one at
`phy_param[0x17]`, and tail-calls the no-op exit hook. Rust has no equivalent
close lifecycle.

## `phy_get_romfunc_addr`

Size `0x98`. Strict status: **NO-REGISTER-EFFECT**.

The function gives the archive `phy_param` address to ROM `phy_param_addr`,
obtains the ROM callback table, saves its pointer in `g_phyFuns`, and replaces
table slots at byte offsets `0, 4, 8, 12, 20, 24, 28, 32, 36, 40, 48` with
the archive I2C, temperature, gain-table, RX-compensation and PA-compensation
functions. It only writes software pointer tables. Rust deliberately owns
these operations directly and has no callback-table ABI.

## `phy_rc_cal_init`

Size `0x36`. Strict status: **BODY-AUDITED**.

The body constructs the exact local values `0x141e1428`, halfword `0x2814`,
and `0x20162824`, passes pointers to those three objects to `phy_rc_cal`, and
returns. There is no other branch. Rust owns the RC-calibration result
transform and the reached cold composition, but the register-relevant ROM
`phy_rc_cal` body remains open under the strict ledger.

## `phy_close_fe_bb_clk`

Size `0x20`. Strict status: **NOT-PORTED**.

The function full-writes zero to `0x20100400`, fresh-reads `0x20100800`,
clears bits 1:0 and writes it, then full-writes zero to `0x20107c80`. Rust has
no close-clock lifecycle operation with this three-write trace.

## `phy_get_chip_version`

Size `0x3c`. Strict status: **NOT-PORTED**.

The function performs two independent 32-bit reads of `0x20715058`. It
computes
`((second_read >> 4) & 5) * 100 + (first_read & 3)`. Result 100 maps to
parameter byte 3, result 101 maps to 4, and every other result is truncated
directly to a byte. It stores the byte at `phy_param[0x1a9]`. Rust has no
owner for these eFuse reads or this parameter publication.

## `phy_i2c_read_check`

Size `0x60`. Strict status: **NOT-PORTED**.

The first finite loop performs exactly 100 reads of block `0x62`, host 1,
register `0x11` and stores their low bytes on the stack. A second 100-iteration
loop prints every saved index/value pair. Rust has no equivalent diagnostic
read surface.

## `phy_rf_init`

Size `0x122`. Strict status: **BODY-AUDITED**.

The direct child order is exact:

1. open FE/BB clocks;
2. enable BBPLL calibration and bias;
3. `phy_open_i2c_xpd_new(1)`, delay 10 microseconds, clear PBus;
4. select I2C clock 8, enable I2C BBPLL and set ADC rate 1;
5. initialize I2C-master, PWDET and FE registers;
6. initialize temperature reads with `(1, phy_param[0x16])`;
7. initialize background TX power;
8. set RC-cal I2C `(3,1,9)`, run `phy_rc_cal_init`, then filter D-cap;
9. read block `0x62`, host 1, register `0x0f` into
   `phy_param[0x18e]`;
10. run `phy_i2c_init1`, RFPLL charge-pump calibration and the 45-word I2C
    command-memory initialization;
11. read block `0x69`, host 0, register 4, bits 3:0; only a zero result calls
    `phy_i2c_sar2_init_code(0x578)`;
12. run `phy_xtal_duty_cal_init(0)`, FE update, and tail-call
    `phy_set_chan_freq_hw_init(2,4)`.

`PhyRfColdInit` represents this cold order, but several strict ROM child
proofs remain open, including RC calibration, RFPLL/XTAL-duty and the target
I2C integration. It is therefore not promoted from direct-body coverage.

## `phy_bb_init`

Size `0x16a`. Strict status: **MISMATCH**.

The body sets bit 2 of `0x20100800` and replaces bits 1:0 of `0x20100028`
with 2. If guard bit 3 at `phy_param + 0xa4` is clear, it performs TXDC,
PWDET, TX-cap, temperature, TX-power, TXDC/PWDET, D-code and TXIQ calibration
in that order, then sets guard bit 3.

The unconditional suffix publishes 32 CFR entries, calls
`phy_bt_tx_gain_init`, publishes PBus memory, samples temperature, runs RXIQ
calibration and RX-table init, brackets `phy_check_rx_sat` and
`phy_set_rx_gain_table(0x985,0)` with the saturation-reset child, initializes
and updates PHY/BB/AGC registers, enables AGC, selects channel 11, clears bits
1:0 of `0x20100028`, conditionally disables Wi-Fi, initializes TX-rate I2C,
and tail-calls `phy_bb_txpwr_track(1)`.

Rust omits the unconditional BT gain child. It also reaches the documented
RX-table parameter defects and the TX/RX calibration PBus timing defects.
Consequently even the default cold branch is not transaction-equivalent.

## `register_chipv7_phy_init_param`

Size `0x94`. Strict status: **NO-REGISTER-EFFECT**.

The function copies exactly 71 bytes from a 128-byte caller profile into
`phy_param`:

- input byte `0x00` to parameter `0x4e`;
- 18 bytes at input `0x02..0x13` to parameter `0x50..0x61`;
- input `0x18` to parameter `0x64`;
- three 14-byte ranges beginning at input `0x19`, `0x27`, and `0x35` to
  parameter `0x6e`, `0x7c`, and `0x8a`;
- nine bytes at input `0x43..0x4b` to parameter `0x65..0x6d`.

Rust `apply_init_data` reproduces every source/destination range.

## `phy_get_rom_ver`

Size `0x0c`. Strict status: **NO-REGISTER-EFFECT**.

The function loads software word `_rom_eco_version` and returns its low four
bits. There are no child or MMIO accesses.

## `phy_rfcal_data_sub_new`

Size `0x64`. Strict status: **NO-REGISTER-EFFECT**.

For exactly 127 words/508 bytes, nonzero input one serializes each
`phy_param` word little-endian to caller offset `0x0c`; zero input one rebuilds
each word from those four caller bytes. Rust `backup_into` and `recover_from`
preserve the same payload offset, length and byte order.

## `phy_rf_cal_data_recovery_new`

Size `0x0a`. Strict status: **NO-REGISTER-EFFECT**.

This is a tail-call wrapper for `phy_rfcal_data_sub_new(record, 0)`. Rust
`recover_from` is the equivalent owned operation.

## `phy_rf_cal_data_backup_new`

Size `0x16`. Strict status: **NO-REGISTER-EFFECT**.

The wrapper calls `phy_rfcal_data_sub_new(record, 1)` and returns zero. Rust
`backup_into` has the same 508-byte data effect.

## `phy_rfcal_data_check_new`

Size `0x7e`. Strict status: **NO-REGISTER-EFFECT**.

The function first calls `phy_set_mac_data(record, version)`, which publishes
the four-byte calibration version and eight-byte chip identity. It sums the
130 little-endian words through record offset `0x207` and complements the
wrapping sum. Check mode subtracts the stored word at `0x208` and returns
whether the result is nonzero. Write mode stores the complement
little-endian at `0x208..=0x20b` and returns zero.

Rust `calibration_record_check_or_write` reproduces the 12-byte header,
130-word checksum geometry, complement, byte order and Boolean result using
explicitly supplied identity words.

## `register_chipv7_phy`

Size `0x1e6`. Strict status: **MISMATCH**.

The parent builds a 128-byte default init profile when input zero is null,
installs the ROM callback table, clears bits 1:0 of `0x20109c18`, forces TX/RX
off, resets/disables frequency hardware and resets I2C. On the first
registration it applies the supplied/default init profile and obtains XTAL
and RF-calibration versions.

Its third input selects calibration-record handling. Mode 1 checks the
caller record; a valid record is recovered and has flag word
`phy_param+0xa4` masked with `0xffff0ddf`. Invalid mode-1 data becomes mode 2.
Other non-recovery modes clear the flag word. It sets calibration clock bit 2
at `0x20100800`, calls `phy_rf_init` then `phy_bb_init`, clears that bit, and
initializes temperature using the inverse of saved flag bits 20 and 5.

After full calibration, non-mode-1 paths back up the 508-byte image and write
the record header/checksum. A nonzero `phy_param[0x9f]` additionally applies
channel offset zero. The common tail disables BBPLL calibration, performs a
final I2C read `(0x63,1,0)`, sets `phy_param[0x25]`, enables hardware
frequency control and releases TX/RX. The function returns the record-check
result.

Rust contains the pure record/checksum transforms but
`PhyRegisterTransition` always selects full calibration and does not compose
check, recovery or backup modes. It additionally inherits the reached
`phy_bb_init` mismatches. The open-only configurable initial channel also
differs from this vendor parent, which fixes the baseband call to channel 11.

## Member conclusion

All archive members now have complete direct-body inventories. `phy_init.o`
confirms that the Rust implementation is a cold full-calibration owner, not a
complete lifecycle replacement: wakeup and shutdown are absent, crystal
selection is fixed, calibration-record modes are uncomposed, and the
baseband parent already contains reached transaction differences. No new
vendor defect was established in this member.
