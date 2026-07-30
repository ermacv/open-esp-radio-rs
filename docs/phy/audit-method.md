# ESP32-S31 complete PHY instruction audit method

This is the binding standard for the complete vendor-to-Rust PHY audit. It
supersedes any interpretation of the earlier cold-Wi-Fi profile audit as global
parity.

## Scope

The audit ledger contains two independently checked populations:

- all 161 externally visible code functions in the 15 code-bearing members of
  `_oracles/libphy.a`, including public wrappers whose names do not start with
  `phy_`;
- all 305 externally visible `phy_*` code functions in
  `_oracles/esp32s31_rev0_rom.elf`.

Every symbol remains a separate ledger entry even when it is a two-byte wrapper,
an alias, or delegates all work to another function. Shared callees may reuse a
completed proof, but they may not be silently counted as audited because one
caller was inspected.

## Meaning of “line by line”

The oracle has no trusted source lines, so the unit of evidence is each
instruction and relocation in the complete symbol body. For every function the
audit records:

1. artifact, member, symbol address/section and byte size;
2. input domains, global pointer use and every accessed `phy_param` range;
3. complete control-flow branches, loop bounds and return paths;
4. every direct call and relocation, in order;
5. every register read/write with address, access width, mask, value source and
   read-modify-write ordering;
6. repeated transaction counts and the branch conditions controlling them;
7. delays, readiness predicates, timeout/failure behaviour and cleanup;
8. non-MMIO state that later changes register values;
9. the exact Rust PAC/HAL/PHY owner, or an explicit absence;
10. a trace-level verdict and the evidence needed to reproduce it.

Disassembling only a parent, matching only the final register value, checking
only one caller profile, or finding a similarly named Rust function is not a
completed audit.

## Register-trace equivalence

For the same input, initial register image and sequence of hardware samples,
Rust must preserve:

- the same reads and writes to the same addresses and widths;
- the same masks, source values and intermediate RMW images;
- the same ordering between register transactions, delays and child calls;
- the same branch-dependent presence or absence of transactions;
- the same finite iteration counts and table publication geometry;
- the same cleanup register operations.

An async Rust state machine may yield between vendor operations. Polling loops
are compared as response-indexed traces: each delivered hardware sample must
drive the same next vendor operation. Scheduling differences do not permit
register operations to be removed, reordered or invented.

## Vendor-defect exception

Rust may differ only when the complete oracle body proves a vendor defect and
the exception is entered in [vendor-defects.md](vendor-defects.md). The audit
must still record:

- the exact vendor trace;
- the condition that reaches the defect;
- the safe Rust replacement;
- why the replacement is the smallest necessary divergence;
- the unaffected successful-path trace.

Convenience, narrower product scope, a passing HIL run or a presumed unused
feature is not a vendor-defect exception.

## Per-function statuses

- **UNREVIEWED**: complete instruction body has not been checked.
- **BODY-AUDITED**: all instructions and direct relocations are recorded, but
  one or more child proofs or Rust trace comparisons remain open.
- **MATCHED**: the complete function and all register-relevant children match
  for every vendor-supported branch and input domain.
- **NOT-PORTED**: no Rust path implements the vendor behaviour.
- **MISMATCH**: Rust exists, but at least one non-defect vendor register trace
  differs.
- **VENDOR-DEFECT-EXCEPTION**: the only difference is a documented and proved
  vendor defect.
- **NO-REGISTER-EFFECT**: the complete function has no direct or transitive
  register effect; its non-MMIO behaviour is still documented.

`BODY-AUDITED` is progress, not parity. `MATCHED` is forbidden until all
branches and register-relevant callees have closed proofs.

## Required evidence artifacts

Each completed entry links to a member audit page containing the disassembly
facts and Rust comparison. Machine-reproducible symbol inventories remain in
[vendor-oracle-inventory.md](vendor-oracle-inventory.md). The aggregate ledger
is [function-audit-ledger.md](function-audit-ledger.md).
