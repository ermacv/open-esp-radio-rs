# Verification reports and evidence

## One typed result

`verify source` and `verify inventory` each emit one typed command result.
Human and TSV views are renderers over that model; JSON and JSONL serialize the
same data. Diagnostics and tracing use stderr and cannot corrupt stdout.

`verify inventory --json-report PATH` persists the complete schema-v4 command
report, including:

- target and gate identity;
- source inventories and every per-function verdict;
- protocol and aggregate summaries;
- execution comparisons where applicable;
- evidence identities and baseline comparison;
- SHA-256 provenance for every reported input;
- report publication status in the command result.

There is no schema-v3 reader and no aggregate-only compatibility report. The
removed line-record and `output::file` protocols also have no compatibility
path.

## Accepted baseline

An accepted baseline is keyed by `(source, symbol)` and records the exact
evidence identity. A regression gate fails when an accepted key disappears or
changes kind/digest. New evidence is reported as an addition and does not hide
a regression elsewhere. Profile evidence binds the parsed scenario, argument
domain, observations, scripted responses, reachability and execution sources.

The baseline is not rewritten during protected verification. First persist the
complete report, including a failing result:

```console
cargo vendor-binary-workbench verify inventory \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/authenticated.run \
  --gate regression --match-floor 104 \
  --json-report /tmp/esp32s31-verification.json
```

Then produce a separate deterministic candidate:

```console
cargo vendor-binary-workbench verify evidence \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --report /tmp/esp32s31-verification.json \
  --candidate /tmp/esp32s31.candidate.evidence

diff -u \
  verification/vendor/targets/esp32s31/baselines/phy.evidence \
  /tmp/esp32s31.candidate.evidence
```

`verify evidence` needs only the public project, the persisted schema-v4
report and its baseline. It does not load vendor artifacts, the run spec or an
analysis backend. Entries are sorted by source and symbol. The command refuses
to overwrite either the accepted baseline or its source report, and reports
the report SHA-256 so a review record can bind the transferred protected-run
result.

## Trust boundary

The workbench analyzes exactly the caller-supplied paths. SHA-256 values in the
report are provenance, not an authenticity decision. Protected CI must
authenticate inputs before invoking the tool and must not execute untrusted
pull-request code with proprietary oracle access.

The evidence digest changes whenever a profile, policy, binding, generated
reference, adapter, comparator, execution engine or another registered proof
source changes. The correct response is review, not silently editing the
accepted baseline in the protected job.
