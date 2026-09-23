#!/usr/bin/env python3
"""
batch-verify.py — Phase 2 批验证 gate

在 commit 前跑，检查本批 staged 改动是否满足 spec 所有硬约束。
不通过则 commit 不得落地（exit 1）。

用法:
    python3 scripts/batch-verify.py docs/batches/C5-AP-06.spec.md
    python3 scripts/batch-verify.py docs/batches/C5-AP-06.spec.md --tests
    python3 scripts/batch-verify.py docs/batches/C5-AP-06.spec.md --json

退出码:
    0 = 全部通过 (commit_allowed=true)
    1 = 有 fail (commit_allowed=false)
    2 = 脚本/输入错误
"""
from __future__ import annotations
import argparse, json, re, subprocess, sys
from pathlib import Path


def run(cmd):
    p = subprocess.run(cmd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def load_spec(path):
    text = Path(path).read_text(encoding="utf-8")
    m = re.search(r"```json\s*\n(.*?)\n```", text, re.DOTALL)
    if not m:
        raise ValueError(f"spec 缺少 ```json 块: {path}")
    return json.loads(m.group(1))


def staged_files():
    rc, out, err = run(["git", "diff", "--cached", "--name-only"])
    if rc != 0:
        raise RuntimeError(f"git diff --cached 失败: {err}")
    return [l for l in out.splitlines() if l.strip()]


def staged_numstat():
    rc, out, err = run(["git", "diff", "--cached", "--numstat"])
    if rc != 0:
        raise RuntimeError(f"git diff --cached --numstat 失败: {err}")
    numstat = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) != 3:
            continue
        a, r, path = parts
        if a == "-" or r == "-":
            numstat.append((0, 0, path))
        else:
            numstat.append((int(a), int(r), path))
    rc, out, err = run(["git", "diff", "--cached", "--name-status"])
    if rc != 0:
        raise RuntimeError(f"git diff --cached --name-status 失败: {err}")
    status_map = {}
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        status_map[parts[-1]] = parts[0]
    result = []
    for add, rem, path in numstat:
        status = status_map.get(path, "?")
        result.append((add, rem, status, path))
    return result


def worktree_dirty():
    rc, out, _ = run(["git", "status", "--porcelain"])
    if rc != 0:
        return []
    dirty = []
    for line in out.splitlines():
        if len(line) < 3:
            continue
        x, y = line[0], line[1]
        path = line[3:]
        if y == "M" and x != "M":
            dirty.append(path)
    return dirty


def count_asserts_staged(file):
    rc, out, _ = run(["git", "show", f":{file}"])
    if rc != 0:
        return -1
    return len(re.findall(
        r"\b(?:assert|assert_eq|assert_ne|debug_assert|debug_assert_eq|debug_assert_ne)!",
        out,
    ))


def check_file_set(spec, files):
    exp = sorted(spec.get("expected_files", []))
    act = sorted(files)
    if exp == act:
        return {"name": "file_set", "status": "pass",
                "detail": f"{len(act)} files, exact match"}
    unexp = sorted(set(act) - set(exp))
    miss = sorted(set(exp) - set(act))
    parts = []
    if unexp: parts.append(f"unexpected={unexp}")
    if miss: parts.append(f"missing={miss}")
    return {"name": "file_set", "status": "fail", "detail": "; ".join(parts)}


def check_line_budget(spec, stat):
    """只算 modified (M) files 的 +/- 行；new files (A) 走 check_new_files_budget"""
    if not stat:
        return {"name": "line_budget", "status": "skip", "detail": "no staged changes"}
    modified = [(a, r, p) for a, r, s, p in stat if s == "M"]
    if not modified:
        return {"name": "line_budget", "status": "skip", "detail": "no modified files"}
    add = sum(a for a, _, _ in modified)
    rem = sum(r for _, r, _ in modified)
    max_a = spec.get("max_lines_added", 10**9)
    max_r = spec.get("max_lines_removed", 10**9)
    ok = add <= max_a and rem <= max_r
    return {"name": "line_budget", "status": "pass" if ok else "fail",
            "detail": f"+{add}/-{rem} (modified only, budget +{max_a}/-{max_r})"}


def check_new_files_budget(spec, stat):
    """算 added (A) files 的总行；spec 字段 max_new_files_lines"""
    if not stat:
        return {"name": "new_files_budget", "status": "skip", "detail": "no staged changes"}
    added = [(a, r, p) for a, r, s, p in stat if s == "A"]
    if not added:
        return {"name": "new_files_budget", "status": "skip", "detail": "no new files"}
    add = sum(a for a, _, _ in added)
    max_a = spec.get("max_new_files_lines", 10**9)
    ok = add <= max_a
    return {"name": "new_files_budget", "status": "pass" if ok else "fail",
            "detail": f"+{add} lines in {len(added)} new file(s) (budget +{max_a})"}


def check_findings_in_diff(spec, files):
    findings = spec.get("findings", [])
    if not findings:
        return {"name": "findings_files_in_diff", "status": "skip",
                "detail": "no findings declared"}
    staged = set(files)
    miss = [f.get("file") for f in findings if f.get("file") not in staged]
    if miss:
        return {"name": "findings_files_in_diff", "status": "fail",
                "detail": f"not staged: {miss}"}
    return {"name": "findings_files_in_diff", "status": "pass",
            "detail": f"{len(findings)} findings covered"}


def check_assertions(spec):
    amin = spec.get("assertions_min")
    if not amin:
        return {"name": "assertions_min", "status": "skip", "detail": "not declared"}
    fails, details = [], []
    for f, mn in amin.items():
        n = count_asserts_staged(f)
        if n < 0:
            fails.append(f"{f}: not staged")
        elif n < mn:
            fails.append(f"{f}: {n} < {mn}")
        details.append(f"{f}:{n}>={mn}" if n >= mn else f"{f}:FAIL")
    if fails:
        return {"name": "assertions_min", "status": "fail", "detail": "; ".join(fails)}
    return {"name": "assertions_min", "status": "pass", "detail": "; ".join(details)}


def check_command(name, cmd, timeout=None):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        rc, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return {"name": name, "status": "fail",
                "detail": f"timeout after {timeout}s"}
    if rc == 0:
        return {"name": name, "status": "pass", "detail": "exit 0"}
    tail = (err or out).strip().splitlines()[-3:]
    return {"name": name, "status": "fail",
            "detail": f"exit {rc}: " + " | ".join(tail)}


def check_worktree_clean(dirty):
    if dirty:
        return {"name": "worktree_clean", "status": "fail",
                "detail": f"unstaged changes: {dirty}"}
    return {"name": "worktree_clean", "status": "pass",
            "detail": "no unstaged changes"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("spec", type=Path)
    ap.add_argument("--tests", action="store_true",
                    help="跑 cargo fmt/check/test-all（慢）")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quiet", action="store_true",
                    help="PASS 时仅输出 commit_allowed 行（FAIL 仍输出完整报告）")
    ap.add_argument("--repo-root", type=Path, default=None)
    args = ap.parse_args()

    if args.repo_root:
        import os; os.chdir(args.repo_root)

    try:
        spec = load_spec(args.spec)
    except Exception as e:
        print(f"[batch-verify] spec 加载失败: {e}", file=sys.stderr)
        return 2

    files = staged_files()
    stat = staged_numstat()
    dirty = worktree_dirty()

    checks = [
        check_file_set(spec, files),
        check_line_budget(spec, stat),
        check_new_files_budget(spec, stat),
        check_findings_in_diff(spec, files),
        check_assertions(spec),
        check_worktree_clean(dirty),
    ]
    if args.tests:
        checks.append(check_command("cargo_fmt", ["cargo", "fmt", "--check"], timeout=60))
        checks.append(check_command("cargo_check_tests", ["cargo", "check", "--tests"], timeout=180))
        checks.append(check_command("test_all", ["bash", "scripts/test-all.sh"], timeout=600))

    any_fail = any(c["status"] == "fail" for c in checks)

    result = {
        "batch_id": spec.get("batch_id", "unknown"),
        "family": spec.get("family", "unknown"),
        "status": "fail" if any_fail else "pass",
        "commit_allowed": not any_fail,
        "checks": checks,
        "staged_files": files,
        "staged_stat": {
            "added": sum(a for a, _, s, _ in stat if s == "M"),
            "removed": sum(r for _, r, s, _ in stat if s == "M"),
        },
    }

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    elif args.quiet and result['commit_allowed']:
        print(f"commit_allowed: {result['commit_allowed']}")
    else:
        print(f"batch: {result['batch_id']}  family: {result['family']}")
        print(f"staged: {len(files)} file(s), "
              f"+{result['staged_stat']['added']}/-{result['staged_stat']['removed']}")
        print()
        for c in checks:
            mark = {"pass": "PASS", "fail": "FAIL", "skip": "SKIP"}[c["status"]]
            print(f"  [{mark}] {c['name']}: {c['detail']}")
        print()
        print(f"commit_allowed: {result['commit_allowed']}")

    return 0 if not any_fail else 1


if __name__ == "__main__":
    sys.exit(main())