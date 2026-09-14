#!/bin/sh
set -eu

if test "$(id -u)" -eq 0; then
    echo "do not run this legacy entry point through sudo" >&2
    echo "run: cargo hil fixture install --provider linux-bluetooth" >&2
    exit 64
fi

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/../../.." && pwd)
cd "$repo_root"
exec cargo hil fixture install --provider linux-bluetooth "$@"
