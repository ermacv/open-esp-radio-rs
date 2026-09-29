#!/bin/bash
# elfcmp.sh OLD.elf NEW.elf: SHA-256 of every allocated section with contents.
set -e
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
sections() { llvm-readelf -S -W "$1" | awk '$0 ~ /\]/ { sub(/.*\] */, ""); n=$1; t=$2; f=$7; if (t=="PROGBITS" && f ~ /A/) print n }'; }
dig() { llvm-objcopy -O binary --only-section="$2" "$1" "$tmp/s"; [ -s "$tmp/s" ] || [ "$(llvm-readelf -S -W "$1" | awk -v n="$2" '$0 ~ /\]/ {sub(/.*\] */,""); if ($1==n) print $5}')" = "000000" ] || { echo "empty dump of $2" >&2; exit 2; }; sha256sum "$tmp/s" | cut -c1-16; }
status=0
for s in $(sections "$1" | sort -u); do
  a=$(dig "$1" "$s"); b=$(dig "$2" "$s")
  if [ "$a" = "$b" ]; then echo "same  $s $a"; else echo "DIFF  $s $a $b"; status=1; fi
done
diff <(sections "$1" | sort) <(sections "$2" | sort) >/dev/null || { echo "DIFF  section sets"; status=1; }
exit $status
