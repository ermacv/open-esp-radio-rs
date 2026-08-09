#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Unsafe is an implementation detail of these audited foundations. The PAC is
# generated; the other leaves deny unsafe by default and reopen it only around
# individually justified operations.
generated_unsafe_leaf="driver/chips/esp32s31/pac"
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

echo "driver unsafe boundary audit passed"
echo "generated_leaf=$generated_unsafe_leaf"
printf 'audited_leaf=%s\n' "${audited_unsafe_leaves[@]}"
