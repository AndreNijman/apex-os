#!/usr/bin/env python3
"""Which task branches still carry work the integration tip does not have.

Three obvious checks are wrong here, and all three have produced a false
answer in this program already:

  git merge-base --is-ancestor  the flow REBASES task branches onto
                                roadmap/v2.2, so every landed branch reads as
                                unmerged. Always wrong, never useful.
  git cherry                    compares patch-ids, so a commit whose
                                conflicts were resolved during the rebase
                                reads as unlanded. Two false alarms on
                                2026-09-06: task/p0-022-agents-help and
                                task/p0-004-policy-split, both fully landed.
  git diff A...B                everything the branch changed since the merge
                                base, landed or not, plus the tip's own later
                                work rendered as deletions. Unreadable.

What actually answers it: take each commit git cherry flags, look at the
distinctive lines it ADDED, and ask whether those lines are on the tip now.
Resolving a conflict reworks context; it does not delete the substance.
"""
import subprocess, sys, os

def sh(*a):
    return subprocess.run(a, capture_output=True, text=True, timeout=120).stdout

def main(repos):
    for repo in repos:
        name = os.path.basename(repo)
        tip = sh('git', '-C', repo, 'log', '--oneline', '-1', 'origin/roadmap/v2.2').strip()
        print(f"  {name}:  tip {tip[:62]}")
        branches = sh('git', '-C', repo, 'for-each-ref', '--format=%(refname:short)',
                      'refs/remotes/origin/task/*', 'refs/remotes/origin/fix/*').split()
        found = False
        for b in branches:
            pending = [l[2:] for l in sh('git', '-C', repo, 'cherry',
                                         'origin/roadmap/v2.2', b).splitlines()
                       if l.startswith('+')]
            if not pending:
                continue
            unlanded = []
            for c in pending:
                diff = sh('git', '-C', repo, 'diff', '--unified=0', f'{c}^', c)
                added = [l[1:].strip() for l in diff.splitlines()
                         if l.startswith('+') and not l.startswith('+++') and len(l) > 25]
                if not added:
                    continue
                step = max(1, len(added) // 10)
                sample = added[::step][:10]
                hits = sum(subprocess.run(
                    ['git', '-C', repo, 'grep', '-qF', '--', line, 'origin/roadmap/v2.2'],
                    capture_output=True).returncode == 0 for line in sample)
                if hits < len(sample) * 0.8:
                    subj = sh('git', '-C', repo, 'log', '--format=%h %s', '-1', c).strip()
                    unlanded.append(f"{subj[:70]}   [{hits}/{len(sample)} added lines on tip]")
            if unlanded:
                found = True
                print(f"    {b[len('origin/'):]}")
                for u in unlanded:
                    print(f"      {u}")
        if not found:
            print("    (nothing — every task branch's content is on the integration tip)")
        print()

if __name__ == '__main__':
    main(sys.argv[1:])
