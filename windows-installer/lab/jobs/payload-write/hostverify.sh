#!/bin/bash
# Runs inside localhost/apex-winlab:latest. /w = winlab scratch, /o = mine.
set -uo pipefail
echo "--- pristine fixture-a.raw must still be the 10:20 baseline ---"
stat -c '%n  %s bytes  mtime %y' /w/fixture-a.raw
echo
echo "--- allocation map of the overlay I am about to verify (not a copy of it) ---"
qemu-img map --output=json /w/payload-write-fixture-a.qcow2 > /o/overlay-map-verify.json
echo "wrote /o/overlay-map-verify.json ($(stat -c %s /o/overlay-map-verify.json) bytes)"
echo
echo "--- qemu-img convert: overlay -> sparse raw, so plain tools can read it ---"
rm -f /o/pw-fixture-a.raw
time qemu-img convert -f qcow2 -O raw /w/payload-write-fixture-a.qcow2 /o/pw-fixture-a.raw
echo "converted: $(stat -c '%s apparent, %b blocks' /o/pw-fixture-a.raw)"
du -h --apparent-size /o/pw-fixture-a.raw; du -h /o/pw-fixture-a.raw
echo
python3 /o/hostverify.py
