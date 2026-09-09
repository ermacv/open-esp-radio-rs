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
        exit 1
        ;;
esac

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/../../.." && pwd)
hostapd_source="$repo_root/target/hil/hostapd/hostapd"
hostapd_provenance="$repo_root/target/hil/hostapd/provenance.json"
hostapd_installed=/usr/local/libexec/open-radio-hostapd
if test ! -x "$hostapd_source" || test ! -s "$hostapd_provenance"; then
    echo "missing hostapd build: run cargo hil fixture install-host" >&2
    exit 1
fi

if test ! -x "$repo_root/target/hil/fixture-build/debug/open-radio-probe"; then
    echo "missing probe helper build: run cargo hil fixture install-host" >&2
    exit 1
fi

install -d -o root -g root -m 0755 /usr/local/libexec
install -d -o root -g root -m 0755 /usr/local/sbin
install -o root -g root -m 0755 "$repo_root/target/hil/fixture-build/debug/open-radio-probe" /usr/local/libexec/open-radio-probe
install -o root -g root -m 0755 "$hostapd_source" "$hostapd_installed"
install -o root -g root -m 0644 "$hostapd_provenance" /usr/local/libexec/open-radio-hostapd.json
install -o root -g root -m 0755 "$script_dir/open-radio-net" /usr/local/sbin/open-radio-net

rm -f /etc/open-radio/hostapd-ht40.conf /etc/open-radio/hostapd-he20.conf

sudoers=/etc/sudoers.d/open-radio-net
{
    echo "$operator ALL=(root) NOPASSWD: /usr/local/libexec/open-radio-probe \"\""
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net ap"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net capabilities"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net identity"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net client"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net observer"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-1"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-6"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-11"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net managed"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net stop"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net status"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net usb-reset"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net kernel-log"
} >"$sudoers"
/bin/chown root:root "$sudoers"
/bin/chmod 0440 "$sudoers"
/usr/sbin/visudo -cf "$sudoers"

echo "installed narrow open-radio HIL permissions for $operator"
