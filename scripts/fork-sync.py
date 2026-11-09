#!/usr/bin/env python3
"""Prepare an upstream merge, then land only the exact verified candidate."""
import argparse
import json
import re
import shlex
import subprocess
import sys


def git(*args, check=True):
    result = subprocess.run(["git", *args], text=True, capture_output=True)
    if check and result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return result


def ancestor(older, newer):
    result = git("merge-base", "--is-ancestor", older, newer, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.strip())
    return result.returncode == 0


def clean():
    if git("status", "--porcelain").stdout.strip():
        raise RuntimeError("Use a clean disposable checkout for fork synchronization")


def resolve_conflicts(command):
    """Let a command resolve the merge in progress; accept it only if nothing is left unresolved."""
    unmerged = git("diff", "--name-only", "--diff-filter=U").stdout.split()
    print("Merge conflicts in: " + ", ".join(unmerged), file=sys.stderr)
    if subprocess.run(shlex.split(command)).returncode:
        raise RuntimeError("The conflict resolver failed")
    git("add", "--", *unmerged)
    left = git("diff", "--name-only", "--diff-filter=U").stdout.split()
    markers = git("grep", "-n", "--cached", "-E", "^(<<<<<<<|>>>>>>>) ", "--", *unmerged, check=False).stdout.strip()
    if left or markers:
        raise RuntimeError("The conflict resolver left conflicts unresolved:\n" + (markers or "\n".join(left)))
    git("commit", "--no-edit")


def prepare(args):
    git("fetch", "--no-tags", args.upstream, args.base)
    upstream = git("rev-parse", "FETCH_HEAD").stdout.strip()
    if ancestor(upstream, "HEAD"):
        return {"changed": False}
    git("switch", "-c", args.branch)
    merged = git("merge", "--no-edit", upstream, check=False)
    if merged.returncode:
        if not args.resolver:
            raise RuntimeError(merged.stderr.strip() or merged.stdout.strip())
        resolve_conflicts(args.resolver)
    sha = git("rev-parse", "HEAD").stdout.strip()
    git("push", "--set-upstream", "origin", args.branch)
    return {"changed": True, "branch": args.branch, "sha": sha}


def land(args):
    if not re.fullmatch(r"[0-9a-f]{40,64}", args.sha):
        raise RuntimeError("The verified candidate must be a full commit SHA")
    git("fetch", "--no-tags", "origin",
        f"refs/heads/{args.base}:refs/remotes/origin/{args.base}",
        f"refs/heads/{args.branch}:refs/remotes/origin/{args.branch}")
    candidate = git("rev-parse", "refs/remotes/origin/" + args.branch).stdout.strip()
    if candidate != args.sha:
        raise RuntimeError("The candidate changed after verification")
    base = "refs/remotes/origin/" + args.base
    if not ancestor(base, candidate):
        raise RuntimeError("Master changed after preparation; prepare and verify a new candidate")
    git("switch", "--detach", base)
    git("merge", "--ff-only", candidate)
    git("push", "origin", "HEAD:refs/heads/" + args.base)
    cleanup = git("push", "origin", "--delete", args.branch, check=False)
    if cleanup.returncode:
        print("The merge landed, but candidate branch cleanup failed: " + cleanup.stderr.strip(), file=sys.stderr)
    return {"landed": True, "sha": candidate, "branchDeleted": cleanup.returncode == 0}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "land"])
    parser.add_argument("--upstream", default="https://github.com/herdrdev/herdr-app.git")
    parser.add_argument("--base", default="master")
    parser.add_argument("--branch", required=True)
    parser.add_argument("--sha")
    parser.add_argument("--resolver", help="command that resolves merge conflicts in the working tree; its result is checked and committed")
    args = parser.parse_args()
    if not args.branch.startswith("automation/upstream-"):
        parser.error("Candidate branches must start with automation/upstream-")
    git("check-ref-format", "--branch", args.branch)
    git("check-ref-format", "--branch", args.base)
    if args.action == "land" and not args.sha:
        parser.error("land requires --sha")
    clean()
    print(json.dumps(prepare(args) if args.action == "prepare" else land(args)))


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
