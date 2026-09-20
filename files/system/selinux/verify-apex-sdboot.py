#!/usr/bin/env python3
"""Read the apex_sdboot rule back out of the BINARY policy.

`semodule -l` proves a module is installed. It does not prove the rule reached
the policy the kernel enforces — a module can install and grant nothing if a
type name changes under it, and the failure would be invisible until a machine
stopped blessing its boots and rolled itself back four boots later.

This lives in its own file rather than as a `python3 -c` inside the
Containerfile deliberately: a Dockerfile eats `\\`+newline before the shell
sees it, and a multi-line assertion written inline is exactly the class of
build-only assertion that has cost this repository whole days
(tests/check-containerfile-assertions.sh exists for it).
"""
import glob
import sys

import setools

WANT = "entrypoint"
SRC = "bootupd_t"
# init_exec_t is systemd-bless-boot; bin_t is /usr/libexec/apex-boot-count.
TARGETS = ("init_exec_t", "bin_t")

candidates = sorted(glob.glob("/etc/selinux/targeted/policy/policy.*"))
if not candidates:
    sys.exit("no binary policy under /etc/selinux/targeted/policy/")
policy_path = candidates[-1]

policy = setools.SELinuxPolicy(policy_path)
for tgt in TARGETS:
    query = setools.TERuleQuery(policy, source=SRC, target=tgt, tclass=["file"])
    perms = set()
    for rule in query.results():
        if rule.ruletype == setools.TERuletype.allow:
            perms.update(str(p) for p in rule.perms)
    if WANT not in perms:
        sys.exit(
            "apex_sdboot did not grant %s -> %s:file %s in %s; got %r"
            % (SRC, tgt, WANT, policy_path, sorted(perms))
        )
    print("apex_sdboot verified in %s: %s -> %s:file %s"
          % (policy_path, SRC, tgt, sorted(perms)))
