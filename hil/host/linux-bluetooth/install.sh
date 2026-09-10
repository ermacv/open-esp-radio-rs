#!/bin/sh
set -eu

if test "$(id -u)" -ne 0; then
    echo "run this installer through sudo" >&2
    exit 1
fi
operator=${SUDO_USER:-}
case "$operator" in
    ''|root|*[!A-Za-z0-9_-]*)
        echo "SUDO_USER must identify the non-root HIL operator" >&2
        exit 1 ;;
esac
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH='' cd -- "$script_dir/../../.." && pwd)
helper="$repository_root/target/debug/open-radio-bluetooth"
test -x "$helper"

# Validate before replacing the active grant. The executable accepts only a
# finite operations with validated adapter/address/delay arguments; it exposes
# no arbitrary HCI API, shell command or output path.
candidate=$(mktemp)
trap 'rm -f "$candidate"' EXIT HUP INT TERM
echo "$operator ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth check --adapter hci*" >"$candidate"
echo "$operator ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-bluetooth connect-reset --adapter hci*" >>"$candidate"
/usr/sbin/visudo -cf "$candidate"
install -d -o root -g root -m 0755 /usr/local/libexec
install -o root -g root -m 0755 "$helper" /usr/local/libexec/open-radio-bluetooth
install -o root -g root -m 0440 "$candidate" /etc/sudoers.d/open-radio-bluetooth
echo "installed finite Bluetooth DTM check and connect-reset for $operator"
