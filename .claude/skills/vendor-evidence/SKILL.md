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
   path, never read it whole. A bare name cites every pinned copy (the
   library's and the ROM's); cite only the copy you reviewed as
   `artifact[member]::symbol` (`libpp[pm.o]::pm_parse_beacon`), the form the
   check prints. Accepting a copy no citation names removes its registration.
3. **After a citation change** run `cargo verification check provenance --chip <chip>`.
   After review, record new fingerprints with
   `cargo verification provenance --chip <chip> --accept NAME`.
4. **After a pin change** run `cargo verification diff --chip <chip> --old A --new B`,
   make production follow the pinned behavior, then re-accept provenance.
5. **Compare compiled production code.** A scenario runs the pinned vendor
   function and the compiled production probe in one Blobray session and
   fails closed: `MATCH`, `DIFF` or `INCOMPLETE`. Never compare against a
   shadow implementation or a duplicated fixture table.
6. **Shards.** The evidence index is derived data, never committed:
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
7. **Hardware cross-checks** are HIL runs: `cargo hil run
   phy-vendor-calibration` (and its `-restart`, `-ieee802154` points) flashes
   the vendor firmware and production alternately and records the
   comparison of `oer-esp32s31-phy-vendor-calibration` as the run bundle's
   typed result. Nothing is written to the vendor evidence index
   `target/verification/<chip>/evidence`.
8. **Commits.** A production change the comparison needs lands in its own
   product-scoped commit, never inside a `blobray` or verification commit.

## Commands (all `run_in_background: true`)

```console
cargo verification fetch <chip>
cargo verification check provenance --chip <chip>
cargo verification scenario <scenario>
cargo verification evidence --chip <chip>
cargo hil plan phy-vendor-calibration
```

`<chip>` is `esp32s31` or `esp32c5`; each has its own pins, provenance
facts, probes and scenarios. The HIL vendor calibration cross-check is
ESP32-S31 only.
