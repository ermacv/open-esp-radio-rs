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

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH= cd -- "$script_dir/../../.." && pwd)
hostapd_source=/tmp/open-radio-hostap-src/hostapd/hostapd
hostapd_installed=/usr/local/libexec/open-radio-hostapd
hil_runner_source="$repository_root/target/debug/open-esp-radio-hil-runner"
if test ! -x "$hostapd_source" && test ! -x "$hostapd_installed"; then
    echo "missing patched hostapd: build $hostapd_source or install $hostapd_installed" >&2
    exit 1
fi
test -x "$hil_runner_source"

install -d -o root -g root -m 0755 /usr/local/libexec
install -d -o root -g root -m 0755 /usr/local/sbin
install -d -o root -g root -m 0755 /etc/open-radio
if test -x "$hostapd_source"; then
    install -o root -g root -m 0755 "$hostapd_source" "$hostapd_installed"
fi
install -o root -g root -m 0755 "$hil_runner_source" /usr/local/libexec/open-radio-hil-runner
/usr/sbin/setcap cap_net_raw+ep /usr/local/libexec/open-radio-hil-runner
install -o root -g root -m 0755 "$script_dir/open-radio-net" /usr/local/sbin/open-radio-net
install -o root -g root -m 0644 "$script_dir/hostapd-ht40.conf" /etc/open-radio/hostapd-ht40.conf
install -o root -g root -m 0644 "$script_dir/hostapd-he20.conf" /etc/open-radio/hostapd-he20.conf

sudoers=/etc/sudoers.d/open-radio-net
{
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net start-ht40"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net start-he20"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net capabilities"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net client"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-1"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-6"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net monitor-11"
    echo "$operator ALL=(root) NOPASSWD: /usr/local/sbin/open-radio-net observer-ht40-1"
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
