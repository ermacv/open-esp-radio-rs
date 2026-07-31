# Revision-zero ROM DC/IQ and RX-DCO audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to the estimator and RX-DCO ROM cluster.
Addresses refer to `_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_abs_temp` | `0x2f825fa2` | `0x0a` | NO-REGISTER-EFFECT | Exact wrapping signed absolute-value transform |
| `phy_linear_to_db` | `0x2f826542` | `0x7c` | NO-REGISTER-EFFECT | Pure logarithmic approximation and exact 16-byte fraction table |
| `phy_iq_est_enable` | `0x2f8289d4` | `0xb4` | MISMATCH | Rust always reads activity after ready; ROM omits that read on the completed observation |
| `phy_iq_est_disable` | `0x2f828a88` | `0x2c` | MATCHED | Exact clear/delay/clear tail |
| `phy_dc_iq_est` | `0x2f828ab4` | `0x84` | MISMATCH | Arithmetic matches the represented domain, but it inherits the estimator-read mismatch and Rust narrows the divisor input to `u16` |
| `phy_txiq_set_reg` | `0x2f827c16` | `0x68` | MISMATCH | Rust masks coefficients without the vendor's signed saturation |
| `phy_pbus_rx_dco_cal` | `0x2f828f44` | `0x228` | MISMATCH | Rust fixes two vendor inputs and inherits the PBus/estimator transaction mismatches |
| `phy_rxdc_est_min` | `0x2f82916c` | `0x98` | MISMATCH | Eight-attempt selection matches; every nested estimate inherits the final extra activity read |
| `phy_pbus_rx_dco_cal_1step` | `0x2f829204` | `0x3ee` | MISMATCH | Rust unsigned DCO state differs from ROM signed halfwords and all PBus/estimator children differ |
| `phy_get_iq_value` | `0x2f8295f2` | `0x36` | NO-REGISTER-EFFECT | Exact signed six-bit/seven-bit packed decode |

All ten direct bodies, branches, loop bounds, global accesses and direct
calls were inspected. No status on this page relies only on a cold caller
profile.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f825fa2 --stop-address=0x2f825fac \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f826542 --stop-address=0x2f8265be \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f8289d4 --stop-address=0x2f828b38 \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f827c16 --stop-address=0x2f827c7e \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f828f44 --stop-address=0x2f8295f2 \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f8295f2 --stop-address=0x2f829628 \
  _oracles/esp32s31_rev0_rom.elf
```

The `phy_linear_to_db` table at `0x2f84832c` is:

```text
00 04 08 0c 10 13 16 19 1c 1f 22 24 27 29 2c 2e
```

## Pure arithmetic leaves

`phy_abs_temp` computes `(value ^ (value >> 31)) - (value >> 31)` with RV32
wrapping arithmetic. This preserves `INT32_MIN` rather than producing a
positive out-of-range result. Rust `wrapping_abs` has the same image.

`phy_linear_to_db(value, scale)` first copies the 16-byte table to its stack.
For unsigned `scale <= 2` it shifts `value` left by `3 - scale`; otherwise it
arithmetically shifts right by `scale - 3`. RV32 uses the low five bits of the
shift count. It subtracts `clz` from 28 and sign-extends that exponent from
eight bits. A positive exponent selects nibble
`(value >> (exponent - 1)) & 15`; a nonpositive exponent becomes zero and
selects `value & 15`. The result is the signed halfword image of
`exponent * 48 + table[nibble]`.

Rust `phy_linear_to_db` reproduces the instructions and table for its `u8`
scale input. The vendor ABI accepts a full RV32 word, so values such as
`0x100` cannot be represented by the Rust API even though their shift-count
image differs from Rust scale zero. This is a pure-domain difference and not a
register effect.

## `phy_iq_est_enable`

The first input is unused. The second input supplies both the estimator
control field and, in its parent, later arithmetic.

ROM:

1. stores halfword zero at `phy_param_rom + 0x1ac`;
2. freshly reads `0x2010044c`, clears bits 27:26, sets bit 26, and writes;
3. freshly reads `0x20100450`, clears bits 20:19, sets bit 20, and writes;
4. freshly reads `0x20100450`, replaces bits 16:2 with the low fifteen bits
   of the second input, and writes;
5. freshly reads the same word, sets bit 0, and writes;
6. delays one microsecond;
7. freshly reads the same word, sets bit 1, and writes;
8. repeatedly reads `0x2010047c`;
9. when its ready bit is clear, reads `0x201008d0`; if bits 21:20 are
   nonzero, increments the wrapping diagnostic halfword at
   `phy_param_rom + 0x1ac`, then returns to step 8;
10. when ready is set, returns immediately without reading `0x201008d0`.

The three configuration RMWs, both enable RMWs, delay and response-indexed
loop are otherwise represented by `PhyDcIqEstimateTransition`.
`sample_iq_estimator_readiness`, however, unconditionally reads both the ready
and activity registers before returning its pair. On every non-ready sample
that matches ROM. On the final ready sample it invents one read of
`0x201008d0`.

The finite Rust deadline is a valid safety replacement for the unbounded wait
documented as `VENDOR-ROBUSTNESS-001`. It does not explain or excuse the
additional successful completion read.

## `phy_iq_est_disable`

ROM freshly reads `0x20100450`, clears bit 1 and writes, delays one
microsecond, then freshly reads the word again, clears bit 0 and writes.
The Rust transition and PAC leaves preserve the two distinct reads, writes
and timer edge. This function is matched independently of the enable-side
finding.

## `phy_dc_iq_est`

Inputs are `(chain, control, output_pointer, mode)`. `chain` has no effect in
the inspected child. ROM calls `phy_iq_est_enable(chain, control)`, then
reads, in order:

1. signed I accumulator `0x20100464`;
2. signed Q accumulator `0x20100468`;
3. signed power accumulator `0x2010046c`.

I and Q use arithmetic shift 6 when mode is zero and shift 4 otherwise, then
signed division by `control + 1`. The results are stored to output words zero
and one. ROM computes:

```text
squares = i*i + q*q
if mode != 0: squares >>= 4
linear = (power_accumulator / (control + 1) << 3) - squares
linear = max(linear, 0)
output[2] = (phy_linear_to_db(linear, 0) + 8) >> 4
```

It tail-calls `phy_iq_est_disable`. Rust preserves this order and wrapping
RV32 arithmetic for a `u16` control. The vendor uses the full input word for
the signed divisor even though the enable child publishes only its low
fifteen bits. Rust therefore differs for control words outside `u16`, and it
also inherits the final-ready activity read from `phy_iq_est_enable`.

## TX-IQ packed decode and register leaf

`phy_get_iq_value(output, packed)` is pure. It extracts bits 14:7, sign
extends their low six-bit image and stores the result byte at `output[0]`.
It sign extends the packed low byte, extracts/sign extends its low seven-bit
image and stores that at `output[1]`. Rust `decode_txiq_coefficient` preserves
the exact `[-32,31]` and `[-64,63]` maps.

`phy_txiq_set_reg(value, kind)` first calls
`phy_get_data_sat(value, limit, -limit)`. A nonzero kind uses limit 31,
freshly reads `0x20100c0c`, replaces bits 5:0 with the saturated low six bits,
and writes once. Zero kind uses limit 63 and replaces bits 12:6 in the same
word with the saturated low seven bits.

Rust `set_tx_iq_gain_coefficient` and `set_tx_iq_phase_coefficient` perform the
same fresh RMW masks, but only mask the supplied `i8`; they do not saturate.
Consequently decoded gain `-32` becomes field image `0x20` in Rust but vendor
clamps it to `-31`, image `0x21`. Decoded phase `-64` similarly becomes
`0x40` instead of vendor-clamped `-63`, image `0x41`. Inputs outside the
`i8` domain are also absent. Other TXIQ transitions sometimes clamp before
calling the PAC leaf, but the archive `phy_set_lb_txiq_new` comparison exposes
the complete packed boundary and cannot be marked matched.

## `phy_rxdc_est_min`

Inputs zero, two and three become estimator control, output pointer and mode;
input one is unused. The body initializes the best power to 100 and makes at
most eight calls to `phy_dc_iq_est(1, control, local, mode)`.

A candidate replaces all three output words only when its power is lower and
either the just-cleared/incremented `phy_param[0x1ac]` halfword is zero or
`phy_param[0x1ae] == 1`. It stops when:

- best power is below 36; or
- at least three measurements have completed and best power is below 48.

If all eight attempts are consumed, ROM overwrites only output word two with
56. Rust preserves the candidate gate, thresholds, attempt count and final
power-only overwrite. Its owned activity counter replaces the parameter
halfword without changing the direct algorithm. The function is nonetheless
a strict mismatch because each child estimate makes the extra completed-state
activity-register read.

## `phy_pbus_rx_dco_cal`

The five inputs are estimator control, a four-halfword configuration pointer,
delay, threshold-mode input and diagnostic input. ROM saves bits 23:22 of
`0x20100434`, clears them with a fresh RMW, and reads
`phy_pbus_rd(1,2)`. It counts the set bits in the returned low six-bit image
and retains the next eight bits.

It publishes the initial I/Q halfwords to selectors 2/3 path 1, then publishes
`0x100` to selectors 2/3 path 2. Up to twelve measurement iterations then:

1. publish current signed-halfword I and Q to path 1;
2. delay by the caller's third input;
3. call `phy_dc_iq_est(1, control, local, 0)`;
4. stop when both signed absolute I/Q estimates are within the threshold;
5. otherwise subtract `phy_get_dco_comp` results for each out-of-threshold
   component and continue.

For zero threshold-mode input the threshold is 2, 4 or 10 for population
classes `0..=1`, `2..=3` and `>=4`. For nonzero input it is 10 on iteration
zero and the low byte of `20 * population` thereafter. The previous estimate
is updated every iteration. The body stores the final signed halfwords,
restores exactly saved bits 23:22 with a fresh RMW, and optionally formats
diagnostics.

`PhyRxDcoTransition` implements only threshold-mode zero and diagnostics off;
its fixed XTAL-duty caller profile uses those values. It also inherits the
PBus pre-publication busy reads and estimator final activity read. Complete
standalone behaviour is therefore a mismatch even though the bounded
twelve-iteration arithmetic matches the represented profile.

## `phy_pbus_rx_dco_cal_1step`

The six inputs control the Wi-Fi/shared bank, radio/baseband stage, estimator
control, a two-halfword DCO pointer, a convergence-byte pointer and gain
index. The body also reads `phy_param[0x10]` bit 3 for diagnostics. It saves
and clears bits 23:22 of `0x20100434`, sign-loads both caller halfwords, reads
`phy_pbus_rd(1,2)`, and counts the low six bits.

Radio stage uses PBus path 2 and at most eight iterations. Baseband stage uses
path 1 and at most twelve. Every iteration publishes both current DCO values.
Radio stage delays ten microseconds and runs one `phy_rxdc_est_min`.
Baseband stage measures after `(1,2,0)` and `(1,2,0x20)`, with a ten
microsecond delay and minimum search after each, then uses the high-minus-low
I/Q deltas. Shared baseband uses threshold 6 and shift 3. Wi-Fi baseband uses
threshold 1 and switches its correction shift from zero to one after the
first two attempts. Wi-Fi gain indexes above one clamp corrections to
`[-5,5]`.

Convergence requires both absolute deltas within threshold and power at most
45. A failed iteration subtracts the selected corrections from the signed
current halfwords and clamps the results to `[0,511]`. Exhausted baseband
search restores the original pair; successful radio search or any converged
search retains the current pair. The function republishes the final I/Q pair,
restores bits 23:22 of `0x20100434`, and returns through the convergence byte.

The diagnostic branch additionally performs six PBus reads, including
selector zero/path one. Thus it reaches the selector-zero physical-address
defect documented in [the PBus audit](rom-pbus-core.md).

Rust `PhyRxDcCalibrationTransition` represents the algorithmic stage/bank
branches, but stores initial/current values as `u16`. ROM begins from signed
`lh` values, so correction subtraction differs for negative halfword images.
The Rust control is also narrowed to `u16`, and every force/minimum child
inherits the lower transaction mismatches. The stale module comment citing
ROM address `0x2f8291ba`, size `0x2f0`, is not oracle provenance: the pinned
symbol is `0x2f829204`, size `0x3ee`.
