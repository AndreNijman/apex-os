#!/usr/bin/env python3
"""Which task branches still carry work the integration branch does not have.

Usage: unlanded.py <integration-ref> <repo> [<repo>...]

Four obvious checks are wrong here, and every one of them has produced a false
answer in this program:

  git merge-base --is-ancestor  the flow REBASES task branches onto the
                                integration branch, so every landed branch
                                reads as unmerged. Always wrong.
  git cherry                    compares patch-ids, so a commit whose conflicts
                                were resolved during a rebase reads as
                                unlanded. Two false alarms in one sweep.
  git diff A...B                everything the branch changed since the merge
                                base, landed or not, plus the integration
                                branch's own later work rendered as deletions.
  are the added lines present?  the heuristic below. It cannot tell "landed in
                                August, then four lines rewritten in September
                                for a better reason" from "fixed twice by
                                different hands", and it read three
                                squash-merged branches as 17 commits of
                                orphaned work. An agent spent a run finding out
                                they had all shipped as PRs #5, #6 and #7.

So the squash check runs FIRST and is exact: if any commit on the integration
branch has the same content as the branch tip across the files that branch
touched, the branch landed, whatever its patch-ids say. Only branches that
survive that get the line-presence heuristic, and its output says it is one.
"""
import subprocess, sys, os

def sh(*a):
    return subprocess.run(a, capture_output=True, text=True, timeout=180).stdout

def squashed_at(repo, branch, integration):
    """The commit on `integration` that carries this branch's content, if any.

    A squash merge leaves no shared patch-id and no ancestry, but it does leave
    a tree: at the squash commit, the files the branch touched look exactly as
    the branch left them. Only commits that touched those files can qualify, so
    this is bounded by the branch's own footprint rather than by history.
    """
    files = sh('git', '-C', repo, 'diff', '--name-only',
               f'{integration}...{branch}').split()
    if not files:
        return None
    candidates = sh('git', '-C', repo, 'log', '--format=%H', '-n', '400',
                    integration, '--', *files).split()
    for c in candidates:
        if subprocess.run(['git', '-C', repo, 'diff', '--quiet', branch, c, '--', *files],
                          capture_output=True).returncode == 0:
            return c
    return None

def main(integration, repos):
    # A report that degrades quietly is worse than one that fails. This script's
    # signature changed once and its caller did not; the result printed a blank
    # tip and "nothing unlanded" for a repository it had never looked at, which
    # is the most dangerous thing a status page can say. So: refuse.
    if not repos:
        sys.exit("unlanded.py: usage: unlanded.py <integration-ref> <repo> [<repo>...]")
    for repo in repos:
        if not os.path.isdir(os.path.join(repo, '.git')) and not os.path.exists(os.path.join(repo, '.git')):
            sys.exit(f"unlanded.py: {repo} is not a git worktree "
                     f"(is the first argument the integration ref, not a repo?)")
        if subprocess.run(['git', '-C', repo, 'rev-parse', '--verify', '--quiet', integration],
                          capture_output=True).returncode != 0:
            sys.exit(f"unlanded.py: {integration!r} does not resolve in {repo}")

    for repo in repos:
        name = os.path.basename(repo)
        tip = sh('git', '-C', repo, 'log', '--oneline', '-1', integration).strip()
        print(f"  {name}:  tip {tip[:62]}")
        branches = sh('git', '-C', repo, 'for-each-ref', '--format=%(refname:short)',
                      'refs/remotes/origin/task/*', 'refs/remotes/origin/fix/*').split()
        found = False
        for b in branches:
            pending = [l[2:] for l in sh('git', '-C', repo, 'cherry',
                                         integration, b).splitlines()
                       if l.startswith('+')]
            if not pending:
                continue

            sq = squashed_at(repo, b, integration)
            if sq:
                subj = sh('git', '-C', repo, 'log', '-1', '--format=%h %s', sq).strip()
                print(f"    {b[len('origin/'):]}  — squash-merged, safe to delete")
                print(f"      landed whole as {subj[:64]}")
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
                    ['git', '-C', repo, 'grep', '-qF', '--', line, integration],
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
    print("  The per-commit lines above are a HEURISTIC: a low count can also mean")
    print("  landed-then-rewritten. Settle any one of them with")
    print("  `git diff <integration> <branch> -- $(git show --name-only --format= <commit>)`")
    print("  before spending an agent on it.")

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2:])
