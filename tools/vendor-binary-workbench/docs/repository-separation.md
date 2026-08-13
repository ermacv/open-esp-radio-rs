# Repository boundary

The standalone Workbench is a reusable OSS product. Its source tree owns the
CLI/TUI, analysis and execution engines, RISC-V backend, semantic contracts,
and register-model/PAC publication formats. It must build without any radio
driver, target project, vendor evidence, SVD publication, or HIL source tree.

Product repositories link target knowledge through a thin host binary:

```text
generic Workbench
        ↑ public HarnessDescriptor callbacks
ESP32-S31 provider ─── production driver
        ↑
ESP32-S31 host binary
```

In `open-esp-radio-rs`, the provider and host live under
`verification/vendor/targets/esp32s31`. The target project, reviewed models,
profiles, dispositions, baselines, SVD, generated PAC, and HIL therefore
remain versioned atomically with the production driver.

Run `scripts/check-standalone` before changing the repository boundary. It
copies only the Workbench tree into a temporary independent workspace and
checks every generic crate. A path dependency escaping that tree is a release
blocker.

Physical history extraction is intentionally the final step, not the first.
It requires a clean integration checkpoint, migrated target-owned regression
coverage and a passing real-project `project check`. Until then the host uses
a workspace path dependency, while `check-standalone` enforces the future
repository boundary on every source change.

The former facade-private target tests are intentionally not copied wholesale.
Their responsibilities now live at the ownership boundary they exercise:

- provider unit tests cover ESP32-S31 ABI models, reviewed summaries and
  semantic adapters;
- product-host black-box tests cover provider dispatch, typed command output
  and validation of the checked-in project/review packs;
- repository tests cover the closed PAC and complete analysis-input producer;
- the resource-limited ESP32-S31 `project check` is the authoritative test of
  private artifacts, profiles, dispositions, evidence baselines and published
  register outputs.

Generic parser, executor and fail-closed behavior stays tested in the
standalone Workbench. This avoids recreating target vocabulary in its facade
solely for white-box access.

Target projects pin their semantic catalogs inside the product tree. The
generic `catalogs/` directory is reusable starter vocabulary, not a runtime
filesystem dependency that product manifests may reach across repositories.

Development uses three branches/worktrees:

- `work/workbench` changes only the standalone tree;
- `work/driver-hil` changes manual driver and HIL code;
- `integration/esp32s31-radio` owns the provider, vendor project, reviewed
  evidence, SVD, generated PAC, baselines, and merges to `main`.

Handoffs are commit hashes, never a shared dirty worktree. Full Workbench
analysis, target builds, and HIL runs are serialized to avoid RAM contention.
