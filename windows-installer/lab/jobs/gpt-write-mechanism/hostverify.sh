#!/bin/bash
set -uo pipefail
echo "--- pristine baselines must be untouched ---"
stat -c '%n  %s bytes  mtime %y' /w/fixture-a.raw /w/golden.raw
echo
for pair in "gptmech-fixture-a.qcow2 gptmech-fa" "gptmech-system.qcow2 gptmech-sys"; do
    set -- $pair
    echo "--- $1 -> /o/$2.raw ---"
    qemu-img map --output=json "/w/$1" > "/o/$2-map.json"
    rm -f "/o/$2.raw"
    qemu-img convert -f qcow2 -O raw "/w/$1" "/o/$2.raw"
    echo "    map $(stat -c %s /o/$2-map.json) B, raw $(du -h /o/$2.raw | cut -f1) on disk"
done
echo
python3 /o/hostverify-gpt.py
