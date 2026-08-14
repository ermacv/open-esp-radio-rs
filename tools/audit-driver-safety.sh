#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Unsafe is an implementation detail of these audited foundations. The raw PAC
# is generated; the closed PAC is safe and is audited like every other safe
# driver crate. The handwritten leaves deny unsafe by default and reopen it
# only around individually justified operations.
generated_unsafe_leaf="driver/chips/esp32s31/pac-raw"
audited_unsafe_leaves=(
    "driver/common/dma"
    "driver/chips/esp32s31/wifi/dma"
    "driver/adapters/embassy/esp32s31-platform"
    "driver/integration/esp32s31/embassy-wifi"
)

is_audited_unsafe_leaf() {
    local candidate="$1"
    local leaf
    for leaf in "${audited_unsafe_leaves[@]}"; do
        if [[ "$candidate" == "$leaf" ]]; then
            return 0
        fi
    done
    return 1
}

mapfile -t manifests < <(find driver -name Cargo.toml -not -path '*/target/*' | sort)
for manifest in "${manifests[@]}"; do
    crate_dir="${manifest%/Cargo.toml}"
    crate_root="$crate_dir/src/lib.rs"
    if [[ ! -f "$crate_root" ]]; then
        echo "driver package has no auditable library root: $manifest" >&2
        exit 1
    fi

    if [[ "$crate_dir" == "$generated_unsafe_leaf" ]]; then
        continue
    fi
    if is_audited_unsafe_leaf "$crate_dir"; then
        if ! rg -q '^#!\[deny\(unsafe_code\)\]$' "$crate_root"; then
            echo "audited unsafe leaf must deny unsafe by default: $crate_root" >&2
            exit 1
        fi
    elif ! rg -q '^#!\[forbid\(unsafe_code\)\]$' "$crate_root"; then
        echo "safe driver crate must forbid unsafe: $crate_root" >&2
        exit 1
    fi
done

# The restricted PAC is an implementation dependency of the HAL. Crates above
# HAL cannot bypass that boundary through either ordinary or dev dependencies.
for manifest in "${manifests[@]}"; do
    case "$manifest" in
        driver/chips/esp32s31/pac/Cargo.toml|driver/chips/esp32s31/pac-raw/Cargo.toml|driver/chips/esp32s31/hal/Cargo.toml)
            continue
            ;;
    esac
    if rg -q 'open-esp-radio-esp32s31-pac([[:space:]]|[[:punct:]])' "$manifest"; then
        echo "driver crate bypasses HAL with a PAC dependency: $manifest" >&2
        exit 1
    fi
done

if rg -n 'test-register-catalog' driver/chips/esp32s31/pac; then
    echo "restricted PAC restored the removed external test register catalog" >&2
    exit 1
fi

# A crate- or module-wide allow would bypass review of the individual
# invariant. Every exception must remain attached to the smallest item or
# expression that needs it.
if rg -n -U '#!\[allow\([^]]*\bunsafe_code\b[^]]*\)\]' driver --glob '*.rs'; then
    echo "driver source contains a broad unsafe_code allowance" >&2
    exit 1
fi

mapfile -t handwritten_sources < <(
    rg --files \
        "${audited_unsafe_leaves[@]}" \
        --glob '*.rs' \
        --glob '!target/**' |
        sort
)
if ! perl -0777 -ne '
    while (/#\[allow\((.*?)\)\]/sg) {
        my $body = $1;
        next unless $body =~ /\bunsafe_code\b/;
        if ($body !~ /\breason\s*=\s*"[^"]+"/) {
            print STDERR "unsafe_code allowance without a reason: $ARGV\n";
            $failed = 1;
        }
    }
    END { exit($failed ? 1 : 0) }
' "${handwritten_sources[@]}"; then
    exit 1
fi

# Only the audited handwritten leaves may reopen the lint. Safe crates use
# `forbid`, but this textual check makes the whitelist violation fail before a
# potentially expensive target build.
mapfile -t all_allowing_sources < <(
    rg -l -U '#\[allow\([^]]*\bunsafe_code\b[^]]*\)\]' \
        driver \
        --glob '*.rs' \
        --glob '!chips/esp32s31/pac/**' || true
)
for source in "${all_allowing_sources[@]}"; do
    allowed=false
    for leaf in "${audited_unsafe_leaves[@]}"; do
        if [[ "$source" == "$leaf/"* ]]; then
            allowed=true
            break
        fi
    done
    if [[ "$allowed" != true ]]; then
        echo "unsafe_code allowance outside the audited leaves: $source" >&2
        exit 1
    fi
done

# PAC owner types are implementation details of the generated/restricted PAC
# and HAL. PHY receives an opaque powered-lifecycle borrow instead; runtime,
# protocol, integration, and application-facing crates use finite HAL
# operations or opaque lifecycle owners.
if rg -n '\b(ColdRadioRegisters|RadioRegisters)\b' \
    driver \
    --glob '*.rs' \
    --glob '!driver/chips/esp32s31/pac/**' \
    --glob '!driver/chips/esp32s31/pac-raw/**' \
    --glob '!driver/chips/esp32s31/hal/**'
then
    echo "PAC owner escaped above the HAL implementation boundary" >&2
    exit 1
fi

if rg -n '\bPhyRegisterAccess\b|\bphy_parts_mut\b|\bregisters_mut\b' \
    driver/chips/esp32s31 \
    --glob '*.rs'
then
    echo "removed powered-PHY compatibility surface was reintroduced" >&2
    exit 1
fi

# The PHY capability is intentionally opaque. A Deref implementation would
# silently restore every public PAC method as an operation available to PHY.
if rg -n 'impl([[:space:]]*<[^>]+>)?[[:space:]]+(core::ops::)?Deref(Mut)?[[:space:]]+for[[:space:]]+PhyHal' \
    driver/chips/esp32s31/hal \
    --glob '*.rs'
then
    echo "PhyHal must not dereference to the PAC owner" >&2
    exit 1
fi

# Removed migration surfaces must stay removed. The `SplitPinned*` names are
# the canonical resource API and do not match these former aliases.
if rg -n '\b(PinnedResources|PinnedDevice|PinnedRadioRunner)\b|register_arena|esp32s31::registers' \
    driver \
    --glob '*.rs'
then
    echo "removed driver compatibility surface was reintroduced" >&2
    exit 1
fi

# Value snapshots may be re-exported, but a crate above the PAC must never
# publicly forward the unique register owner under either its original name
# or a module alias.
if rg -n 'pub use .*open_esp_radio_esp32s31_pac.*(RadioRegisters|ColdRadioRegisters| as registers)' \
    driver \
    --glob '*.rs'
then
    echo "public PAC owner re-export was introduced" >&2
    exit 1
fi

# Hiding the dependency is insufficient if a public HAL signature still asks
# callers to provide the PAC owner. Validation entry points acquire their
# owner internally and production callers receive only opaque HAL capabilities.
if rg -n 'pub (unsafe )?fn [^(]+\([^)]*(RadioRegisters|ColdRadioRegisters)|pub fn new\([^)]*(RadioRegisters|ColdRadioRegisters)' \
    driver/chips/esp32s31/hal/src \
    --glob '*.rs'
then
    echo "HAL public API exposes a PAC owner parameter" >&2
    exit 1
fi

echo "driver unsafe boundary audit passed"
echo "generated_leaf=$generated_unsafe_leaf"
printf 'audited_leaf=%s\n' "${audited_unsafe_leaves[@]}"
