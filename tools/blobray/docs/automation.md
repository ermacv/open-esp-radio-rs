# CLI automation

Use one typed result on stdout and opt into runtime error diagnostics on stderr:

```console
blobray-generic project files --project path/to/vendor-project.toml \
  --format json --diagnostic-format json --quiet
```

`--format` selects command results. `--diagnostic-format` selects runtime error
presentation independently; its default remains human. JSON diagnostics have
`schema_version: 1` and a `diagnostic` object containing the diagnostic code,
message, causes, help and source labels supplied by the error. A failed command
does not emit an empty or successful result on stdout. Use `--quiet` to suppress
tracing and progress when consuming stderr as a JSON error document. Parser
errors and provider initialization failures occur before this runtime boundary
and retain their human CLI diagnostics.

Exit 0 means the command completed successfully. Exit 2 may mean a completed
negative result or a command-line usage error: a typed stdout result distinguishes
the former. Exit 1 means runtime failure. Inventory commands such as `project
files` may succeed while reporting missing prerequisites; inspect the typed
report and use command-specific deny flags when appropriate. An intentionally
closed stdout reader does not panic or interrupt the command's work; its normal
result status is preserved.

## Launching the selected host

Build the selected host with the optimized profile. The limiter accepts an
explicit executable, including paths relative to the current directory:

```console
cargo build --profile blobray -p blobray --bin blobray-generic
BLOBRAY_BINARY=target/blobray/blobray-generic \
  tools/blobray/scripts/run-limited project analyze --project path/to/vendor-project.toml
```

Without `BLOBRAY_BINARY`, the launcher retains the repository's ESP32-S31 host
default. Both watchdog and systemd modes resolve the selected file to an
absolute executable and preserve argument boundaries. Resource limits remain
the same for generic and product hosts.

Next actions carry a logical `argv` whose first element is `blobray`, an absolute
`working_directory`, and the required project-resolution overrides. The logical
name is stable action identity, not a guarantee that a program named `blobray`
is installed on PATH. Bind actions to the same selected host and limiter used
by the caller. For example, given one action object from a report:

```python
assert action["argv"][0] == "blobray"
result = subprocess.run(
    [str(limiter.resolve()), *action["argv"][1:],
     "--format", "json", "--diagnostic-format", "json", "--quiet"],
    cwd=action["working_directory"],
    env={**os.environ, "BLOBRAY_BINARY": str(host.resolve())},
    capture_output=True,
    check=False,
)
```

The caller supplies `host` and `limiter`; resolve their paths before changing
working directory. Execute argument arrays directly, without shell parsing.
An empty action list denotes work requiring external input or a review decision.
Neither the absence of a finding nor a successful inspection proves that a
previous blocker was resolved; use the report's explicit finding query state.

## Composing reusable packs

`project configure` validates the entire candidate before replacing the manifest.
Repeat `--ecosystem-pack` to replace the selected list with several packs in the
specified order. Duplicate paths are rejected. `--no-ecosystem-pack` clears the
list; `--check` verifies the requested composition without publishing a change.

```console
blobray-generic project configure --project vendor-project.toml \
  --ecosystem-pack packs/runtime.toml --ecosystem-pack packs/vendor.toml
```

Library clients use `configure_project` with `ProjectConfigureRequest` and receive
the same typed `ProjectConfigureReport`; they do not depend on CLI argument types
or parse rendered text. Relative paths are resolved from the caller's directory.
