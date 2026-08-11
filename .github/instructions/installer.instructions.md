---
applyTo: "installer/**,files/scripts/spike-e/**,docs/m4-install-runbook.md"
---

# Installer and disk-safety review

Installer regressions can destroy a user's existing OS. Review every path adversarially and fail closed.

- No disk write may occur before the final explicit destructive confirmation. Disk, target partition, ESP, mode, username, hostname, password presence, image reference, and required tools must all be validated first.
- Never infer a destructive target from list order, `/dev/sdX` naming, model text, or size alone. Exclude live media, resolve parent disks, reject target/ESP identity, reject partitions from another disk, and re-check identities immediately before writing.
- Whole-disk and partition modes are distinct contracts. Partition mode must preserve every non-target partition and shared ESP content. Never use a bootc option that replaces an incumbent shared ESP. Preserve existing boot entries and report any BootOrder change.
- The confirmation screen must accurately label every affected partition as erased, kept, or shared and require deliberate typed confirmation. No kickstart, unattended default, timeout, Enter key, or UI fallback may bypass it.
- Account creation must complete before the first normal boot can lock the user out. Validate account fields before erasure, hash/transport passwords safely, never log secrets, and verify the resulting account, groups, shell, and authentication configuration.
- Secure Boot enrollment must preserve the no-Secure-Boot path and never expose private signing material. Validate the public key and enrollment artifacts. A skipped enrollment must be explicit, not caused by parser truncation or missing input.
- Errors must identify the failed stage, preserve logs, avoid claiming success, and leave a recovery shell where designed. Do not continue after mount, format, bootloader, account, image verification, or install failures.
- Test guard ordering, not just guard existence: malformed input must fail before any block device is opened for writing. Test GUI pages at minimum supported resolutions and ensure controls/fields are visible, not merely that a PNG exists.
- Any change touching disk selection, formatting, mounting, boot entries, account creation, confirmation, or answer parsing requires a regression test in `installer/test-installer.sh` or a safer isolated VM test.
