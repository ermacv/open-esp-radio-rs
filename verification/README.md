# Compiled vendor verification

This tree owns reviewed, shareable vendor-analysis projects and their compiled
comparison harnesses. It contains no production driver behavior, HIL board
scenarios or private vendor artifacts.

```text
verification/vendor/
  knowledge/espressif/           reusable ecosystem vocabulary
  chips/esp32s31/                reusable chip identity and compiled providers
  projects/
    esp32s31/                   investigation composition, provider and host
      probes/                   isolated workspace: three library/ELF pairs
      profiles/ dispositions/ baselines/ replays/
      analysis/ evidence/ revisions/
    esp32c5/                    portability fixture
```

The generic engine lives in [`tools/blobray`](../tools/blobray/README.md).
Each project selects its own chip/applicability context and compiled providers;
its `target.toml` specifies the architecture/ABI rather than a lab board.
Declarative knowledge and executable reconstructions remain separate dependency
boundaries. The [ESP32-S31 project](vendor/projects/esp32s31/README.md) links its
concrete host to the generic engine through an explicit provider registry.

Reviewed hardware models and production PAC publication policy belong to
[`registers`](../registers/README.md). The
[source-only publication](../registers/esp32s31/publication/README.md) selects
those inputs independently of binary investigation. Probes retain compiled
production entry points for comparison; they do not implement driver behavior
or execute [HIL scenarios](../hil/README.md).

```console
cargo blobray project doctor --project verification/vendor/projects/esp32s31/vendor-project.toml
```

Artifact paths and authentication remain caller-owned. Checked configuration
must not select private input through hard-coded local paths. Old generated
analysis and local bindings remain ignored after a directory migration; they
are not moved into the reviewed source tree or relabelled as fresh evidence.

Blobray owns comparison truth within each declared claim. The only path from
comparison evidence to product readiness is the independent
[verification and qualification contract](../docs/VERIFICATION_AND_QUALIFICATION.md).
