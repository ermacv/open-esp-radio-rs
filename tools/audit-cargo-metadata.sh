#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if (($# != 0)); then
    echo "usage: $0" >&2
    exit 2
fi

cd "$repo_root"

# Discover workspace roots through Cargo instead of maintaining a second list
# beside the manifests. Include non-ignored additions so a moved independent
# workspace is checked before staging. Git ignores exclude local build trees;
# the explicit private-input exclusion applies even if a file was force-added.
declare -A workspace_manifests=()
while IFS= read -r -d '' manifest; do
    # A local removal remains in the index until commit; audit the manifests
    # that actually remain in the source tree.
    [[ -f "$manifest" ]] || continue
    workspace_manifest="$(
        cargo locate-project \
            --workspace \
            --manifest-path "$manifest" \
            --message-format plain
    )"
    case "$workspace_manifest" in
        "$repo_root/Cargo.toml" | "$repo_root/"*) ;;
        *)
            echo "Cargo workspace escaped the repository: $workspace_manifest" >&2
            exit 1
            ;;
    esac
    workspace_manifests["$workspace_manifest"]=1
done < <(git ls-files --cached --others --exclude-standard -z -- \
    'Cargo.toml' '**/Cargo.toml' ':(exclude)_oracles/**')

if ((${#workspace_manifests[@]} == 0)); then
    echo "no source Cargo manifests found" >&2
    exit 1
fi

mapfile -t sorted_workspace_manifests < <(
    printf '%s\n' "${!workspace_manifests[@]}" | LC_ALL=C sort
)

for workspace_manifest in "${sorted_workspace_manifests[@]}"; do
    relative_manifest="${workspace_manifest#"$repo_root/"}"
    echo "checking locked Cargo metadata: $relative_manifest"
    cargo metadata \
        --manifest-path "$workspace_manifest" \
        --locked \
        --format-version 1 \
        >/dev/null
done

echo "locked Cargo metadata passed for ${#sorted_workspace_manifests[@]} workspace(s)"
