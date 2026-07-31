# Line audit: `libphy.a[phy_rx_cal.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines thirteen external code functions. Every instruction,
relocation, branch, finite loop and byte of the 46-byte constant object was
inspected. All thirteen functions are strictly closed: three
**NO-REGISTER-EFFECT**, one **NOT-PORTED**, and nine **MISMATCH**.

Two reached cold-path defects are particularly important. The composed Rust
RX-gain DC owner holds the second conditional PBus work-mode pulse for one
microsecond instead of the vendor's two. The Rust RX-saturation owner clears
debug mode and enables work mode, but discards the condition that requires
both subsequent delays and the pulse. Neither difference is a vendor-defect
exception.

The member's `.rodata` is exactly:

- four DCO halfwords, all `0x0100`;
- eight Wi-Fi calibration gains:
  `0x0040, 0x0041, 0x0043, 0x006e, 0x0078, 0x0079, 0x007b, 0x007f`;
- eleven shared-radio gains:
  `0x0040, 0x0041, 0x0042, 0x0043, 0x006e, 0x0078, 0x0079, 0x007b,
  0x027f, 0x017f, 0x007f`.

## `phy_pbus_rx_dco_cal_1step_new`

Size `0x4a2`. Strict status: **MISMATCH**.

This is an eight-argument RX-DC correction search. It first reads diagnostic
bit 3 from `phy_param[0x10]`, saves bits 23:22 of `0x20100434`, and clears
those two bits with a fresh-read RMW. It reads `phy_pbus_rd(1, 2)` and counts
the set bits in the low six bits. A zero stage input selects radio path 2 and
eight attempts; every nonzero stage input selects baseband path 1 and twelve
attempts.

Each attempt first publishes the working I/Q halfwords with selectors 2 and 3
on the selected path. Radio calibration then delays 10 microseconds and calls
`phy_rxdc_est_min`. Baseband calibration additionally measures selector
`(1,2)` at values zero and `0x20`, with a 10-microsecond delay and
`phy_rxdc_est_min` after each force. The high-minus-low result is reduced by
the caller's two signed reference deltas.

The correction threshold and shift branches are exact:

- radio threshold is `max(popcount, 2) - 1`, and the shift is
  `max(popcount, 2) - 2`;
- shared-radio baseband uses threshold 6 and shift 3;
- Wi-Fi baseband uses threshold 1, shift zero for attempts zero and one, and
  shift one thereafter;
- a correction that is still zero becomes the sign of the delta when the
  corresponding low estimate has absolute value below 50, otherwise it uses
  the shifted low estimate;
- Wi-Fi baseband gain indices above one clamp each correction to `[-5, 5]`;
- power 45 or greater suppresses both baseband corrections.

Convergence requires both absolute deltas to be at most the threshold and
power below 46. Every updated component is clamped to `[0, 0x1ff]`. A
baseband search that exhausts its twelve attempts restores the two original
halfwords; radio exhaustion keeps the last clamped values. The function
finally forces both selected-path components and restores only bits 23:22 of
`0x20100434`.

`PhyRxDcCalibrationTransition` reproduces this reached search for valid
nine-bit initial DCO images. It is not complete for the standalone input
domain: the vendor loads both caller halfwords with signed `lh` instructions
before doing the subtract-and-clamp arithmetic, while Rust stores them as
`u16` and converts them to positive `i32`. For example, initial `0xffff` can
clamp toward zero in the vendor and toward `0x1ff` in Rust for the same
measurement response. Rust also narrows the estimator control input to
`u16`. These are ordinary input-domain mismatches; the cold callers' `0x0100`
images are unaffected.

## `phy_set_lb_txiq_new`

Size `0x32`. Strict status: **MISMATCH**.

The body calls `phy_get_iq_value(input, stack_pair)`, sign-loads byte zero and
calls `phy_txiq_set_reg(value, 1)`, then sign-loads byte one and calls
`phy_txiq_set_reg(value, 0)`. There are no other branches or accesses.

Rust has the same packed-coefficient decode and orders the TX-IQ gain write
before the phase write in `PhyRxIqGainTransition`. The closed
`phy_txiq_set_reg` proof exposes two boundary mismatches: vendor saturates
decoded gain `-32` to `-31` and phase `-64` to `-63`, while the Rust PAC leaf
only masks the original values. The resulting field images are respectively
`0x21` versus `0x20` and `0x41` versus `0x40`.

## `phy_set_rx_gain_cal_iq_new`

Size `0x25e`. Strict status: **MISMATCH**.

The four inputs control a temporary I2C branch, the tone/RX-IQ selector, the
output halfword pointer and diagnostics. For nonzero input zero the body reads
block `0x67`, host 1, register 3, bit 2; writes that bit to zero; and restores
the saved bit before returning. Zero input omits all three operations.

The common trace is:

1. set loopback gain `(0, 0x43, 0x20)`;
2. select channel D-code 6 and apply `phy_param_u16[0xe6]` through
   `phy_set_lb_txiq_new`;
3. run `phy_pbus_rx_dco_cal(4000, [0x100; 4], 10, 0, diagnostic-bit-3)`;
4. start with attenuation `0x30` and baseband gain `0x20`;
5. for at most two passes, force `(1,1,0)`, `(1,1,0x1f9)`, start tone with
   the caller's second input, enable the IQ estimator with control `0x3ff`,
   read total power from `0x2010046c >> 7`, disable the estimator and stop
   tone path 1;
6. if power exceeds `0x20000`, first reduce baseband gain to zero, otherwise
   add `0x18` to attenuation; if power is below `0x1000`, subtract `0x18`;
   attenuation is saturated to `[0, 0x78]`;
7. repeat the loopback/RX-DCO setup for the adjusted gain;
8. call `phy_get_rfcal_rxiq_data(second_input, attenuation, debug)` and store
   its halfword result through input two.

The diagnostic branch adds three PBus result reads and formatting only.

Rust `PhyRxIqGainTransition` implements the cold input-zero profile and fixes
the selector to `0x80`. It has no representation of the nonzero input-zero
I2C save/clear/restore trace and cannot reproduce arbitrary second-input
selectors. The omitted operations are register effects, so the complete
function is a mismatch even though the reached cold arithmetic is represented.

## `phy_bt_rx_mx_dgain`

Size `0x2a`. Strict status: **NO-REGISTER-EFFECT**.

The body constructs eleven stack bytes and returns by unsigned input index:
indices `0..=8` return zero, index 9 returns 4, and index 10 or any larger
input returns 7. There is no global, MMIO, or child access. Rust
`shared_mixer_dgain` reproduces the map.

## `phy_rxdc_fine_delta`

Size `0x110`. Strict status: **MISMATCH**.

The setup forces, in order, `(0,1,0)`, `(2,1,0x100)`,
`(3,1,0x100)`, `(2,2,0x100)`, and `(3,2,0x100)`. It then processes the six
codes `0x00, 0x20, 0x30, 0x38, 0x3c, 0x3e`. For each code it forces
`(1,2,code)` and calls `phy_pbus_rx_dco_cal_1step_new` in radio mode with
control `0x800`, the running `[0x100,0x100]` pair, gain index zero,
`phy_param + 0x1e0` as the reference pointer, and a caller-local convergence
byte. Output row zero is `[0,0]`; rows one through five are wrapping
halfword differences from the first calibrated pair.

`PhyRxGainDcTransition` preserves the five setup commands, six codes, running
configuration and six output rows. The fixed child profile does not reach the
signed-initial mismatch above. It does reach the now-closed ROM PBus and
estimator differences: every force command adds a pre-publication busy read,
and every completed minimum estimate adds an activity-register read.

## `phy_rxdc_est_delta`

Size `0xda`. Strict status: **MISMATCH**.

The exact PBus prefix is `(0,1,0)`, `(2,1,0x100)`, `(3,1,0x100)`,
`(2,2,0x100)`, `(3,2,0x100)`, then `(1,2,0)`. After a 10-microsecond delay it
calls `phy_rxdc_est_min(0x800, 1, low, 0)`, forces `(1,2,0x20)`, delays
another 10 microseconds, and repeats the call into `high`. It stores
`high.i-low.i` and `high.q-low.q` as two caller halfwords.

The `ReferenceSetup`, `ReferenceDelay`, `ReferenceMinimum` and
`ReferenceHigh` states in `PhyRxGainDcTransition` reproduce this order.
The closed `phy_rxdc_est_min` proof shows that each nested estimate performs
an additional completed-state activity-register read in Rust. Its PBus
commands also inherit the extra pre-publication busy read.

## `phy_set_rx_gain_cal_dc_new`

Size `0x2cc`. Strict status: **MISMATCH**.

Input zero selects one of two banks: zero selects the eight-entry Wi-Fi table;
every nonzero value selects the eleven-entry shared-radio table. The second
input is unused. Inputs two and three are output/base pointers.

The common prefix sets bit 2 of `0x20100800`, sets bits 6:5 of
`0x20100424`, programs RFPLL frequency `0x9b4` using `phy_param[0x4f]`,
enters PBus debug mode, enables RX with argument zero, and enables RX then TX
clocks.

The shared-radio branch additionally:

- reads `(1,1)`, sets bit 1 and forces the result back;
- clears block `0x67`, host 1, register 3, bit 2;
- uses the eleven-entry table and, after `phy_pbus_set_rxgain`, replaces the
  low three bits of the parameter-ROM byte at offset 2 with
  `phy_bt_rx_mx_dgain(index)`;
- restores the I2C bit to one after the loop.

The Wi-Fi branch first calls
`phy_rxdc_fine_delta(phy_param + 0x1e0)` and uses the eight-entry table.
Both branches call `phy_rxdc_est_delta`, then iterate their exact table
length. Each row resets radio I/Q to `0x100`; later Wi-Fi rows reuse the
caller's base pair, while the other baseband setup is `0x100`. It calls
`phy_pbus_set_rxgain(gain << 12)` and
`phy_pbus_rx_dco_cal_1step_new` in baseband mode. Wi-Fi row zero also runs a
radio-mode step and writes the caller's base pair.

Cleanup disables RX and TX clocks, calls `phy_pbus_xpd_rx_off`, calls
`phy_pbus_workmode`, and clears bits 6:5 of `0x20100424`.

`PhyRxGainDcTransition` represents both fixed bank invocations and the table
geometry. Its cleanup, however, requests a one-microsecond delay after the
second conditional PBus work-mode pulse. Complete ROM
`phy_pbus_force_mode(0)` requires two microseconds. This affects the reached
cold graph and makes the register/delay trace non-equivalent.

## `phy_rfrx_gain_index_new`

Size `0x74`. Strict status: **NO-REGISTER-EFFECT**.

The function copies both constant gain tables to its stack. Zero input zero
selects the eight-entry Wi-Fi table; nonzero selects the eleven-entry shared
table. It returns the first index whose halfword equals input one, or the
selected table length when no value matches. Its only child is `memcpy`; it
has no register effect.

## `phy_xtal_duty_cal`

Size `0x392`. Strict status: **MISMATCH**.

The ordered preparation is:

1. save `0x20100434`;
2. clear block `0x61`, host 1, register 7, bit 5 and write
   `phy_param[0x19e]` to register 10;
3. call `phy_set_rf_freq_offset(phy_param[0x4f],
   u16(frequency_code - 5), 0)`;
4. start tone `(1,0x80,0,0,0,0)`, enable RX then TX clocks, enter PBus debug
   mode, and enable RX with `0xf0`;
5. force `(0,1,0x43)`, `(1,1,0x38)`, and `(1,1,0x189)`;
6. clear bits 23:22 of `0x20100434`, run
   `phy_pbus_rx_dco_cal(4000,[0x100;4],10,0,0)`, and restore only those two
   saved bits.

The search writes every duty candidate from `0x20` through `0x3e`, inclusive,
and delays 20 microseconds after every write. It takes four signed 64-bit
`phy_get_rx_sig_pwr(12)` samples, computes their mean, and uses
`mean * 2 / 3` and `mean * 3 / 2` as inclusive bounds. Each outlier is
replaced once and, if that replacement is also outside the bounds, a second
time. Thus each candidate performs four through six measurements. The four
accepted values are averaged; the first candidate with the strictly lowest
average wins, so ties preserve the earlier duty.

The tail restores register 10 to `phy_param[0x19e]`, stops tone with
`(0,0x80,0x28,0,0,0)`, disables RX then TX clocks, turns RX off and enters
PBus work mode. The second input gates formatting only and does not change the
register trace.

`XtalDutyPassTransition` preserves all 31 candidates, replacement branches,
signed 64-bit arithmetic, first-on-tie rule, setup and cleanup. Its PBus pulse
uses the correct two-microsecond delay. The closed RX-DCO and PBus child
proofs nevertheless establish additional successful-path reads, so the
complete function is already a mismatch; remaining child work cannot restore
strict equivalence.

## `phy_xtal_duty_cal_init`

Size `0x74`. Strict status: **MISMATCH**.

The wrapper reads block `0x61`, host 1, register 9, bits 5:0 into
`phy_param[0x19e]`; clears register 7 bit 5; calls
`phy_xtal_duty_cal(0x988, debug)` into `phy_param[0x19f]`; then calls
`phy_xtal_duty_cal(0x9b0, debug)` into `phy_param[0x1a0]`.

`XtalDutyCalibrationTransition` has the same read, redundant path clear,
frequency order and three parameter outputs. The absent debug argument only
controls formatting. Both calls inherit the closed mismatch above.

## `phy_get_xtal_duty`

Size `0x36`. Strict status: **NO-REGISTER-EFFECT**.

For unsigned input at most `0x967`, the function returns `0x11`. Otherwise it
performs a wrapping 16-bit subtraction of `0x975`: results at most `0x26`
return `phy_param[0x19f]`, and all others return `phy_param[0x1a0]`. This means
the gap `0x968..=0x974` selects the outer value. Rust
`phy_frequency_xtal_duty` reproduces every boundary and the wrapping
arithmetic.

## `phy_xtal_duty_set`

Size `0x3e`. Strict status: **NOT-PORTED**.

The function obtains the duty through `phy_get_xtal_duty`, clears block
`0x61`, host 1, register 7, bit 5, and writes the complete duty byte to
register 10. Rust has the pure selector and the I2C leaves, and uses register
10 during calibration and frequency-memory construction, but has no runtime
owner that performs this two-write operation for a supplied frequency.

## `phy_check_rx_sat`

Size `0x76`. Strict status: **MISMATCH**.

The function loads the four `0x0100` DCO halfwords, enters PBus debug mode,
calls `phy_pbus_xpd_rx_on(0)`, applies the DCO image, and delays 5
microseconds. It then performs exactly 100 fresh 32-bit loads from
`0x201008d0`; bits 21:20 being nonzero increments a 16-bit counter. If the
final count is nonzero it stores one at `phy_param[0x1ae]`; a zero count does
not clear an existing flag. It finally calls `phy_pbus_workmode`.

`PhyRxSaturationTransition` preserves the eleven expanded PBus commands, the
delay, all 100 response-indexed samples and the one-way parameter flag. Its
MMIO binding calls `configure_work_mode` but discards the returned
`wifi_baseband_is_enabled` condition. When that condition is true, vendor
`phy_pbus_force_mode(0)` additionally delays 1 microsecond, asserts the
work-mode pulse, delays 2 microseconds, and clears the pulse. Rust performs
none of those four edges in this owner. The successful cold trace is
therefore a mismatch when the baseband is enabled.

## Member conclusion

The main cold algorithms and table geometry are substantially present, but
this member is not register-trace equivalent. Reached RX-gain DC cleanup has
the wrong pulse duration, reached RX-saturation cleanup omits the conditional
pulse entirely, the nested estimator/PBus bindings add successful-path reads,
and two standalone calibration helpers expose wider register or signed-input
domains than their Rust owners. No difference in this member qualifies as a
proved vendor defect.
