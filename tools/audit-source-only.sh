#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
image_pid=""

cleanup() {
    if [[ -n "$image_pid" ]] && kill -0 "$image_pid" 2>/dev/null; then
        kill -- "-$image_pid" 2>/dev/null || true
        wait "$image_pid" 2>/dev/null || true
    fi
    rm -rf -- "$audit_dir"
}
trap cleanup EXIT

cd "$repo_root"
command -v jq >/dev/null
command -v llvm-nm >/dev/null
command -v llvm-cxxfilt >/dev/null
command -v setsid >/dev/null

# Every tracked Cargo package must resolve through a checked-in lockfile at its
# actual workspace boundary. This also covers the independently buildable HIL,
# example, product-integration, and probe workspaces without reading local or
# private vendor inputs.
tools/audit-cargo-metadata.sh

# The public ESP32-S31 examples are independent workspaces so the root
# workspace check cannot detect a stale composition API. Build every shipping
# example against the real target before accepting a source-only checkout.
tools/check-esp32s31-examples.sh

# The optimized final-image link is the longest mostly independent stage. Build
# the Host runner once, then let its isolated runtime/bootstrap target trees use
# the machine while the root-workspace audits run. A failing foreground gate
# terminates this background owner through the EXIT trap.
cargo build \
    --quiet \
    --locked \
    --offline \
    -p open-esp-radio-hil-runner
image_build_log="$audit_dir/final-image-build.log"
setsid env -u ESP_HAL_ROOT \
    "$repo_root/target/debug/open-esp-radio-hil-runner" \
    image build performance >"$image_build_log" 2>&1 &
image_pid="$!"
echo "source-only audit: final HIL image build running concurrently (pid=$image_pid)"

# Research resumes only from a warning-free workspace. Keep this fail-closed:
# adding a new target or crate automatically subjects it to the same budget.
# The driver-only disallowed-method policy is compiled separately below because
# host/HIL traffic pacing has legitimate busy-wait loops.
cargo clippy --workspace --all-targets -- -D warnings -A clippy::disallowed-methods

# Production radio code must yield through its executor/platform instead of
# burning a CPU in spin_loop. Clippy resolves the actual called method, so this
# does not depend on source spelling or aliases.
mapfile -t production_manifests < <(find driver -name Cargo.toml -print | sort)
test "${#production_manifests[@]}" -gt 0
workspace_packages="$audit_dir/workspace-packages.json"
cargo metadata \
    --format-version 1 \
    --locked \
    --offline \
    --no-deps >"$workspace_packages"
workspace_package_arguments=()
standalone_manifests=()
for manifest in "${production_manifests[@]}"; do
    manifest_absolute="$(realpath "$manifest")"
    package="$(jq -r --arg manifest "$manifest_absolute" \
        '[.packages[] | select(.manifest_path == $manifest) | .name]
        | if length == 0 then "" elif length == 1 then .[0] else error("manifest identifies multiple packages") end' \
        "$workspace_packages")"
    if [[ -n "$package" ]]; then
        workspace_package_arguments+=(--package "$package")
    else
        standalone_manifests+=("$manifest")
    fi
done

# All-features is already the maximal feature union, so Cargo can lint every
# root-workspace driver library in one dependency-graph traversal without
# weakening an individual package check.
cargo clippy \
    --quiet \
    --locked \
    --offline \
    "${workspace_package_arguments[@]}" \
    --target "$target_triple" \
    --lib \
    --all-features \
    --no-deps \
    -- \
    -D clippy::disallowed-methods

# Excluded/independent driver workspaces cannot share a Cargo invocation with
# the root workspace, but remain discovered automatically and checked exactly.
for manifest in "${standalone_manifests[@]}"; do
    metadata="$audit_dir/standalone-$(basename "${manifest%/Cargo.toml}").json"
    cargo metadata \
        --format-version 1 \
        --locked \
        --offline \
        --no-deps \
        --manifest-path "$manifest" >"$metadata"
    package="$(jq -er --arg manifest "$(realpath "$manifest")" '
        [.packages[] | select(.manifest_path == $manifest) | .name]
        | if length == 1 then .[0] else error("manifest does not identify exactly one package") end
    ' "$metadata")"
    cargo clippy \
        --quiet \
        --locked \
        --offline \
        --manifest-path "$manifest" \
        --package "$package" \
        --target "$target_triple" \
        --lib \
        --all-features \
        --no-deps \
        -- \
        -D clippy::disallowed-methods
done

tools/audit-driver-safety.sh
tools/audit-driver-architecture.sh

# Validate the checked-in register sources without requiring disposable vendor
# analysis output. Unreviewed observations are the explicit research backlog,
# not invalid source: this gate validates their schema, ownership, evidence and
# publication boundaries without turning incomplete research into a build
# failure. Completion claims use the stricter `--deny-unreviewed` policy.
cargo blobray project configure \
    --project verification/vendor/targets/esp32s31/vendor-project.toml \
    --check
cargo blobray registers validate \
    --project verification/vendor/targets/esp32s31/vendor-project.toml

# When the optional analysis report is already available, also prove that the
# complete generated SVD/PAC/binding publication is reproducible. Its absence
# only disables this deeper local check; it does not weaken register-source
# validation above.
review_scope_report="verification/vendor/targets/esp32s31/generated/findings/review-scopes.json"
if [[ -f "$review_scope_report" ]]; then
    cargo blobray project publish \
        --project verification/vendor/targets/esp32s31/vendor-project.toml \
        --check
else
    echo "source-only audit: optional review-scope report absent; skipping publication reproducibility check"
fi

build_messages="$audit_dir/phy-build.jsonl"
cargo build \
    -p open-esp-radio-esp32s31-phy \
    --lib \
    --release \
    --target "$target_triple" \
    --message-format=json-render-diagnostics >"$build_messages"

artifact="$(
    jq -ser '
        [
            .[]
            | select(.reason == "compiler-artifact")
            | select(.target.name == "open_esp_radio_esp32s31_phy")
            | select(any(.target.kind[]; . == "lib"))
            | select(.profile.test == false)
            | .filenames[]
            | select(endswith(".rlib"))
        ]
        | unique
        | if length == 1 then .[0] else error("build did not emit exactly one PHY rlib") end
    ' "$build_messages"
)"
test -f "$artifact"

write_symbols() {
    local mode="$1"
    local output="$2"

    llvm-nm "--$mode" --just-symbol-name "$artifact" 2>/dev/null |
        while IFS= read -r symbol; do
            # llvm-nm prints archive member headings beside symbol names.
            case "$symbol" in
                "" | *:) continue ;;
            esac
            printf '%s\n' "$symbol"
        done |
        sort -u >"$output"
}

write_symbols undefined-only "$audit_dir/undefined"
write_symbols defined-only "$audit_dir/defined"
comm -23 "$audit_dir/undefined" "$audit_dir/defined" >"$audit_dir/external"

# The final artifact may refer to its source-only HAL/register dependencies and to
# compiler/core support only. Public pure-model `Debug` implementations retain
# `core::fmt` leaves in an rlib even when the final image does not call them.
# Radio ROM or vendor archive symbols still fail closed.
is_allowed_external_symbol() {
    local symbol="$1"
    local subject
    local trait
    local leaf

    case "$symbol" in
        open_esp_radio_esp32s31_hal::* | \
            open_esp_radio_esp32s31_pac::* | \
            open_esp_radio_esp32s31_pac_raw::* | \
            core::fmt::* | \
            __divdi3 | __udivdi3 | memcmp | memcpy | memmove | memset)
            return 0
            ;;
    esac

    if [[ "$symbol" == "<"* ]]; then
        subject="${symbol#<}"
        subject="${subject%%>::*}"
        trait="${subject##* as }"
        case "$subject" in
            open_esp_radio_esp32s31_hal::* | \
                open_esp_radio_esp32s31_pac::* | \
                open_esp_radio_esp32s31_pac_raw::* | \
                core::fmt::*) return 0 ;;
        esac
        case "$trait" in
            open_esp_radio_esp32s31_hal::* | \
                open_esp_radio_esp32s31_pac::* | \
                open_esp_radio_esp32s31_pac_raw::* | \
                core::fmt::*) return 0 ;;
        esac
    fi

    case "$symbol" in
        core::*)
            leaf="${symbol##*::}"
            case "$leaf" in
                panic* | len_mismatch_fail*) return 0 ;;
            esac
            ;;
    esac
    return 1
}

while IFS= read -r symbol; do
    demangled="$(printf '%s\n' "$symbol" | llvm-cxxfilt)"
    if ! is_allowed_external_symbol "$demangled"; then
        echo "unexpected external symbol in source-only radio rlib: $demangled" >&2
        exit 1
    fi
done <"$audit_dir/external"

is_forbidden_radio_symbol() {
    case "$1" in
        phy_wifi_get_tx_gain | \
            register_chipv7_phy | \
            g_phyFuns | \
            phy_param | \
            esp_wifi_* | \
            pp_* | \
            net80211_*) return 0 ;;
        *) return 1 ;;
    esac
}

llvm-nm --just-symbol-name "$artifact" 2>/dev/null |
    while IFS= read -r symbol; do
        case "$symbol" in
            "" | *:) continue ;;
        esac
        if is_forbidden_radio_symbol "$symbol"; then
            echo "radio ROM/vendor ABI symbol survived source-only build: $symbol" >&2
            exit 1
        fi
    done

# Verify the exact normal/build dependency closure selected for the compiled
# PHY target. Cargo package IDs and dependency kinds are used directly.
phy_metadata="$audit_dir/phy-metadata.json"
cargo metadata \
    --format-version 1 \
    --locked \
    --offline \
    --manifest-path driver/chips/esp32s31/phy/Cargo.toml \
    --filter-platform "$target_triple" >"$phy_metadata"
phy_manifest="$(realpath driver/chips/esp32s31/phy/Cargo.toml)"
allowed_phy_packages='[
    "critical-section",
    "open-esp-radio-dma",
    "open-esp-radio-esp32s31-coex",
    "open-esp-radio-esp32s31-hal",
    "open-esp-radio-esp32s31-ieee802154-irq",
    "open-esp-radio-esp32s31-pac",
    "open-esp-radio-esp32s31-pac-raw",
    "open-esp-radio-esp32s31-phy",
    "vcell"
]'
unexpected_packages="$(
    jq -r \
        --arg manifest "$phy_manifest" \
        --argjson allowed "$allowed_phy_packages" '
        def closure($id; $edges):
            $id, (($edges[$id] // [])[] | closure(.; $edges));

        ([.packages[] | select(.manifest_path == $manifest) | .id]
            | if length == 1 then .[0] else error("PHY manifest does not identify exactly one package") end) as $root
        | (.resolve.nodes
            | map({
                key: .id,
                value: [.deps[] | select(any(.dep_kinds[]; .kind != "dev")) | .pkg]
            })
            | from_entries) as $edges
        | (.packages | map({key: .id, value: .}) | from_entries) as $packages
        | [closure($root; $edges)] | unique[]
        | $packages[.]
        | select(.name as $name | all($allowed[]; . != $name))
        | .name
    ' "$phy_metadata"
)"
if [[ -n "$unexpected_packages" ]]; then
    echo "unexpected package in source-only PHY dependency graph:" >&2
    echo "$unexpected_packages" >&2
    exit 1
fi

# Join the exact final-image build started above. A sibling esp-hal checkout is
# a useful HIL development override, but using it here would both mutate the
# embedded lockfile and make this policy gate machine-dependent.
if ! wait "$image_pid"; then
    image_pid=""
    cat "$image_build_log" >&2
    echo "source-only audit: final HIL image build failed" >&2
    exit 1
fi
image_pid=""
cat "$image_build_log"

runtime_elf="target/hil/esp32s31/psram-code-psram-data-psram-stack-performance/cargo/runtime/$target_triple/release/open-esp-radio-hil-esp32s31-runtime"
test -f "$runtime_elf"

# The linker script exposes absolute ESP32-S31 ECO0 ROM symbols even when no
# call references them, so symbol-table matching is insufficient. Decode all
# executable sections and reject statically resolved jumps/calls into the
# pinned radio API table or the contiguous radio implementation body. System
# ROM outside these ranges (for example ets_printf) remains permitted.
cargo blobray advanced image audit-targets \
    --target-spec verification/vendor/targets/esp32s31/target.toml \
    --artifact "$runtime_elf" \
    --forbid 'esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc' \
    --forbid 'esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0'

echo "source-only radio audit passed: rlib=$artifact runtime=$runtime_elf"
