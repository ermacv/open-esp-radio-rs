---
name: vendor-evidence
description: Use when work touches vendor comparison or provenance in this repository — verification/<chip>/artifacts.toml pins, cargo verification fetch, SOURCE( ) citations, verification/<chip>/facts/provenance.toml, check provenance, vendor-diff, vendor-provenance --accept, vendor scenarios and probes, evidence shards, or MATCH/DIFF/INCOMPLETE results.
---

# Vendor pins, provenance and comparison evidence

Read first (about 3k tokens): [verification levels](../../../verification/README.md),
[pinned vendor artifacts](../../../verification/esp32s31/README.md#pinned-vendor-artifacts)
and the `SOURCE` rules in [recovered tables](../../../docs/source-policy.md#recovered-tables-and-coefficients).
Scenario authors also read the
[vendor verification path](../../../docs/verification-and-qualification.md#vendor-verification-path).

## Checklist

1. **Pins.** `verification/<chip>/artifacts.toml` is the only pin of each
   vendor archive, ROM ELF and SDK firmware. Fetch with
   `cargo verification fetch <chip>`; never commit the artifacts, dumps or
   `_oracles/`. Other hashes record where a fact was observed; they are not pins.
2. **Cite.** A recovered fact names its vendor function in a
   `SOURCE(<chip>):` block (one recogniser, `oer_vendor_provenance::citation`,
   which `cargo tidy check` uses too) (a chip-neutral path names its chips). The cited
   function is registered with its reviewed fingerprint in
   `verification/<chip>/facts/provenance.toml`; grep that file by explicit
   path, never read it whole. A function several pinned artifacts define
   (the ROM's copy and a library's, which may handle the same register bits
   differently) is cited by the copy the fact was read from, as
   `artifact[member]::symbol` (`libpp[pm.o]::pm_parse_beacon`,
   `rom[]::pm_tbtt_process`), the form the check prints; a bare name of one
   fails the check. A bare name cites a function's only copy. Coverage
   decisions are exempt until #655. Accepting a copy no citation names
   removes its registration.
3. **After a citation change** run `cargo verification check provenance --chip <chip>`.
   After review, record new fingerprints with
   `cargo verification provenance --chip <chip> --accept NAME`.
4. **Which copy.** `cargo verification copies --chip <chip>` shows the
   functions whose ROM and library copies differ in MMIO, with the copies
   our facts cite; read a `differs` pair's fact against the copy it cites.
5. **After a pin change** run `cargo verification diff --chip <chip> --old A --new B`,
   make production follow the pinned behavior, then re-accept provenance.
6. **Compare compiled production code.** A scenario runs the pinned vendor
   function and the compiled production probe in one Blobray session and
   fails closed: `MATCH`, `DIFF` or `INCOMPLETE`. Never compare against a
   shadow implementation or a duplicated fixture table.
7. **Shards.** The evidence index is derived data, never committed:
   `cargo verification evidence --chip <chip>` computes it for the checkout
   into `target/verification/<chip>/evidence`, and qualification reads it
   there; a stale index holds no evidence (`EVIDENCE-DIR stale`) until
   it is computed again. The format, the one
   writer and the source policy are `oer-vendor-evidence`
   (`verification/evidence`); its producers are the Blobray scenario engine
   and the host stands (`verification/<chip>/host/<name>`, shard
   `<name>-host`, written by the stand's own `shard` command). A shard never
   records a file of a report package.
   On a branch, CI's `vendor-evidence` compares the index with `main`'s:
   a claimed vendor root that is gone, or whose comparisons reach fewer
   blocks or branch directions, fails it unless
   `verification/<chip>/decisions/accepted-losses.toml` accepts it with a
   reason (`[[retired]]`, or `[[narrowed]]` with the reach kept). A
   `[[narrowed]]` entry the comparisons reach beyond again is an error too:
   a change that widens such a root removes or updates its entry
   ([comparison with main](../../../verification/README.md)).
8. **Hardware cross-checks** have no HIL scenario now: the
   `phy-vendor-calibration*` scenarios were deleted with the images they ran
   on (#762, tag `legacy-wifi-2026-10-16`) and return with the HIL
   architecture of #761. The comparison itself stays in
   `oer-esp32s31-phy-vendor-calibration`. Nothing is written to the vendor
   evidence index `target/verification/<chip>/evidence`.
9. **Commits.** A production change the comparison needs lands in its own
   product-scoped commit, never inside a `blobray` or verification commit.

## Commands (all `run_in_background: true`)

```console
cargo verification fetch <chip>
cargo verification check provenance --chip <chip>
cargo verification scenario <scenario>
cargo verification evidence --chip <chip>
```

`<chip>` is `esp32s31` or `esp32c5`; each has its own pins, provenance
facts, probes and scenarios. The HIL vendor calibration cross-check is
ESP32-S31 only.
