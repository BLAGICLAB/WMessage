#!/usr/bin/env bash
# install-git-hook.sh — 生成 .githooks/pre-commit（幂等）
#
# 策略：全量生成，不 patch。每次跑输出逐字节相同。
# 所有权语义：本脚本拥有 .githooks/pre-commit，用户自定义逻辑请勿直接编辑，改为扩展本模板。
# 安全阀：检测现有 hook 是否本工具生成（看 marker），不是则拒绝覆盖。

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

HOOK=".githooks/pre-commit"
MARK_BEGIN="# >>> batch-verify gate (managed) >>>"
MARK_END="# <<< batch-verify gate (managed) <<<"

# --- 安全阀：marker-based 所有权检测 ---
if [[ -f "$HOOK" ]] && ! grep -qF "$MARK_BEGIN" "$HOOK"; then
    echo "[install-hook] ERROR: $HOOK 已存在且非本工具生成，拒绝覆盖" >&2
    echo "[install-hook] 手工处理（备份+删除或改名）后重试" >&2
    exit 1
fi

# --- 全量生成 ---
cat > "$HOOK" <<'HOOK_TEMPLATE'
#!/usr/bin/env bash
cd "$(git rev-parse --show-toplevel)" || { echo "[pre-commit] 无法定位 repo root"; exit 1; }

# >>> batch-verify gate (managed) >>>
# src/ 或 src-tauri/ 有 staged 改动 → 强制跑 batch-verify.py
# BATCH_SPEC 未设 = 拒绝 commit
if ! _staged="$(git diff --cached --name-only 2>/dev/null)"; then
    echo "[pre-commit] git diff --cached 失败（repo 损坏？index lock？submodule 状态异常？）" >&2
    echo "[pre-commit] 此检查是 1.6 cd toplevel 兜底之外的独立防线：防御 hook cwd 正确但 git 状态异常的场景" >&2
    exit 1
fi
_needs_gate=0
if printf '%s\n' "$_staged" | grep -qE '^(src-tauri|src)/'; then
    _needs_gate=1
fi
if [[ "$_needs_gate" -eq 1 ]]; then
    if [[ -z "${BATCH_SPEC:-}" ]]; then
        echo "" >&2
        echo "[pre-commit] batch-verify gate: src/ 或 src-tauri/ 有 staged 改动，但 BATCH_SPEC 未设" >&2
        echo "[pre-commit] 本批必须显式声明 spec:" >&2
        echo "[pre-commit]   BATCH_SPEC=docs/batches/<BATCH_ID>.spec.md git commit ..." >&2
        echo "" >&2
        exit 1
    fi
    if [[ -d "$BATCH_SPEC" ]]; then
        echo "[pre-commit] BATCH_SPEC 是目录（不是 spec 文件）: $BATCH_SPEC" >&2
        exit 1
    fi
    if [[ ! -f "$BATCH_SPEC" ]]; then
        echo "[pre-commit] BATCH_SPEC 文件不存在: $BATCH_SPEC" >&2
        exit 1
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        echo "[pre-commit] python3 not found in PATH" >&2
        exit 1
    fi
    echo "[pre-commit] batch-verify gate: spec=$BATCH_SPEC" >&2
    if ! python3 scripts/batch-verify.py --quiet "$BATCH_SPEC"; then
        echo "" >&2
        echo "[pre-commit] batch-verify gate: FAIL — 见上方输出" >&2
        echo "[pre-commit] commit 已拦截。修正后重试；调 spec 需 reviewer 批准" >&2
        exit 1
    fi
    echo "[pre-commit] batch-verify gate: PASS" >&2
fi
# <<< batch-verify gate (managed) <<<

# versioned pre-commit → scripts/test-fast.sh
exec "$(dirname "$0")/../scripts/test-fast.sh"
HOOK_TEMPLATE

chmod +x "$HOOK"
echo "[install-hook] 生成 $HOOK"
bash -n "$HOOK" && echo "[install-hook] 语法 OK"
