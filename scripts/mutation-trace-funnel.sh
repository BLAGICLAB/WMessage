#!/usr/bin/env bash
# 变异测试：把 trace 观测面 + 溢出回灌的关键决策逐个改坏，看现有单测抓不抓得住。
#
# 读法：
#   KILLED   = 变异被某个单测抓住（好，说明该处有真实约束在守）
#   SURVIVED = 变异活着跑完全程（差，说明这里没有测试兜底——补测试，不是改代码）
#   BUILD_ERR= 变异编译不过（等于被编译器抓住，也算 KILLED，但要记下来人工看）
#
# 用法：bash scripts/mutation-trace-funnel.sh            # 全跑
#        bash scripts/mutation-trace-funnel.sh M4 M7      # 只跑指定变异
#
# 全程只改工作区文件、每个变异跑完即 git checkout 还原，不产生 commit。
# 因此开跑前要求 src-tauri/src 是干净树——否则还原会覆盖真改动。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAURI="$ROOT/src-tauri"
SRC="$TAURI/src"
cd "$ROOT" || exit 1

# 变异目录必须干净，否则还原会覆盖真改动
if ! git diff --quiet -- "$SRC" 2>/dev/null; then
  echo "✗ src-tauri/src 有未提交改动，先提交或 stash 再跑变异测试（还原会覆盖）"
  exit 1
fi

PASSED=0; KILLED=0; SURVIVED=0; BUILDERR=0
RESULTS=()
# 空 = 全跑；给了 id 就只跑那几个（复核单个变异时省十几分钟编译）
SELECT=("$@")

# run_mutant <id> <说明> <文件相对 src> <行号> <sed表达式> <目标测试子串>
run_mutant() {
  local id="$1" desc="$2" file="$3" line="$4" expr="$5" target="$6"
  if [ ${#SELECT[@]} -gt 0 ]; then
    case " ${SELECT[*]} " in *" $id "*) ;; *) return ;; esac
  fi
  local path="$SRC/$file" tmp
  tmp="$(mktemp)"

  # 变异结果写临时文件再覆盖（不用 sed -i：BSD/GNU 的 -i 语义不同，还会留下 .bak）
  sed -e "${line}s${expr}" "$path" > "$tmp"
  if cmp -s "$path" "$tmp"; then
    printf '  %-4s %-10s ⚠️  变异未生效（行号或模式对不上，检查脚本）\n' "$id" "NO-OP"
    RESULTS+=("$id|NO-OP|$desc")
    rm -f "$tmp"
    PASSED=$((PASSED+1))
    return
  fi
  mv -f "$tmp" "$path"

  local out status
  if [ -n "$target" ]; then
    out=$(cargo test --manifest-path "$TAURI/Cargo.toml" --lib -- "$target" 2>&1)
  else
    out=$(cargo test --manifest-path "$TAURI/Cargo.toml" --lib -- trace_sink:: overflow:: 2>&1)
  fi
  status=$?

  local verdict
  if [ $status -eq 0 ]; then
    verdict="SURVIVED"
    SURVIVED=$((SURVIVED+1))
  elif printf '%s' "$out" | grep -qE '^error(\[E[0-9]+\]|: could not compile)'; then
    # 只认 rustc 的错误码 / "could not compile"；cargo 在测试失败后打的
    # `error: test failed` 不是编译错误，撞上它会把 KILLED 全误判成 BUILD_ERR
    verdict="BUILD_ERR"
    BUILDERR=$((BUILDERR+1))
  else
    verdict="KILLED"
    KILLED=$((KILLED+1))
  fi

  # 失败摘要给人看，别只给一个词
  local detail=""
  if [ "$verdict" = "KILLED" ]; then
    detail=$(printf '%s' "$out" | grep -E '^test .* \.\.\. FAILED' | head -3 | sed 's/^test //; s/ \.\.\..*//' | paste -sd, -)
    [ -n "$detail" ] || detail=$(printf '%s' "$out" | grep -E 'assertion|panicked' | head -1 | sed 's/^ *//')
  elif [ "$verdict" = "BUILD_ERR" ]; then
    detail=$(printf '%s' "$out" | grep -E '^error(\[E[0-9]+\]|: could not compile)' | head -1)
  fi

  printf '  %-4s %-10s %s\n' "$id" "$verdict" "$desc"
  [ -n "$detail" ] && printf '        └─ %s\n' "$detail"

  RESULTS+=("$id|$verdict|$desc")
  git -C "$ROOT" checkout -- "$path" 2>/dev/null || true
}

echo "══ 变异测试：trace 观测面 + 溢出回灌 ══"
echo

# ── 溢出回灌漏斗 ────────────────────────────────────────────────
# 行号锚点 2026-10-24 刷新（漏斗判定已抽成 apply_output_cap 泛型函数，见
# bot_model_loop.rs；锚点漂移时 run_mutant 会报 NO-OP，照 grep 重对即可）。
run_mutant M1 "L1/L2 取小写成取大 → run_python 不再溢出" \
  bot_model_loop.rs 574 '/\.min(global_cap_bytes)/.max(global_cap_bytes)/' effective_cap

# M2 原表达式 unwrap_or(usize::MAX) 是等价变异：后面同一行的 .min(global) 会
# 把 MAX 钳回 global，任何行为测试都杀不死它。改成 unwrap_or(0)——未注册工具
# 的兜底既不能放大也不能归零，既有测试（mcp_unknown → 全局 cap）即可杀死。
run_mutant M2 "未注册工具兜底归零 → MCP 工具全部转存" \
  bot_model_loop.rs 573 '/\.unwrap_or(global_cap_bytes)/.unwrap_or(0)/' effective_cap

run_mutant M3 "SEPS 归零 → 节选凭空超出 cap 4 字符" \
  bot_model_loop.rs 663 '/const SEPS: usize = 4;/const SEPS: usize = 0;/' preview_keeps_head

run_mutant M4 "指针长度按字符算 → 中文指针把正文预算算少" \
  bot_model_loop.rs 664 '/notice\.len()/notice.chars().count()/' preview_

run_mutant M5 "边界 > 改成 >= → 恰好等于 cap 的结果被白转存一次" \
  bot_model_loop.rs 632 '/if text\.len() > cap/if text.len() >= cap/' preview_

run_mutant M6 "落盘失败兜底返回空串 → 工具结果凭空消失" \
  bot_model_loop.rs 615 '/cap_tool_output(&text, cap_bytes)/String::new()/' preview_

run_mutant M7 "白名单里的溢出目录路径写坏 → 指针成废纸" \
  bot_fs.rs 285 '|crate::overflow::root(app)|crate::overflow::root(app).join("nope")|' preview_

run_mutant M8 "保留期判定方向反过来 → 只删新文件、老的永远留着" \
  overflow.rs 126 '/mtime < cutoff/mtime > cutoff/' sweep

run_mutant M9 "保留期天数去掉 max(0) 下限" \
  overflow.rs 103 '/\.max(0)//' sweep

run_mutant M10 "指针里的行数按字符算 → read_text_file 的 offset 报错" \
  overflow.rs 73 '/text\.lines()\.count()/text.chars().count()/' pointer_text

# ── trace 观测面（commit 7e474ff）────────────────────────────────
run_mutant M11 "钳制标记预算不再扣标记长度 → span 字段超上限" \
  trace_sink.rs 148 '/\.saturating_sub(marker\.len())//' clamped_text

run_mutant M12 "满队列只记日志不计数 → 健康检查看不到观测缺口" \
  trace_sink.rs 107 '/let n = DROPPED\.fetch_add(1, Ordering::Relaxed) + 1;/let n = 0;/' full_queue_counts

echo
echo "── 汇总 ──"
echo "  KILLED=$KILLED  SURVIVED=$SURVIVED  BUILD_ERR=$BUILDERR  NO-OP=$PASSED"
echo
SURVIVORS=$(printf '%s\n' "${RESULTS[@]}" | grep 'SURVIVED' | cut -d'|' -f1 | paste -sd' ' -)
if [ -n "$SURVIVORS" ]; then
  echo "  存活变异：$SURVIVORS"
  echo "  ↑ 这些位置当前没有测试兜底。补测试，不要改代码——它们本来就是正确实现。"
fi
