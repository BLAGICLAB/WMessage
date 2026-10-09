#!/bin/bash
# 老板 22:30 拍板的 wmessage 自进化 pipeline 健康检查
#
# 五环断言 + 输出 OK/FAIL。
# 用法：bash scripts/health-check.sh
#
# 跑一次真实交互后立刻 bash scripts/health-check.sh 看结果。
# 五环全 OK = 正常运转。任一 FAIL → 找对应行号定位。

set -u

# 探多个候选 data dir，按 wmessage.db 存在者为准（便携模式锁定 exe_dir）
# 老板 12:50 拍板：health-check 必须支持便携模式，否则全报告错路径
CANDIDATE_DATA_DIRS=(
  "./src-tauri/target/debug"              # dev 便携模式（target/debug/wmessage.db）
  "./target/debug"                        # 兜底
  "./evolution"                            # 项目根，prod 默认
  "$HOME/Library/Application Support/wmessage"
  "$HOME/Library/Application Support/com.renshi.wmessage"
)
EVOLUTION_DIR=""
for d in "${CANDIDATE_DATA_DIRS[@]}"; do
  if [ -f "$d/wmessage.db" ]; then
    EVOLUTION_DIR="$d"
    break
  fi
done
if [ -z "$EVOLUTION_DIR" ]; then
  # fallback 到项目根（避免脚本中断）
  EVOLUTION_DIR="./evolution"
fi
PROPOSALS="$EVOLUTION_DIR/evolution-proposals.jsonl"
CHANGES="$EVOLUTION_DIR/evolution-changes.jsonl"
SYNTHETIC_DIR="./evolution.synthetic"

OK=0
FAIL=0

ok() { printf "  \033[32m✓\033[0m %s\n" "$1"; OK=$((OK+1)); }
fail() { printf "  \033[31m✗\033[0m %s\n" "$1"; FAIL=$((FAIL+1)); }

echo "=== ① 真实交互（手动） ==="
echo "  你是否刚跑了一次任务 / 对话？（Y/N，跳过 = N）"
echo "  此步手动检查，跳过即可。"

echo ""
echo "=== ② evolution-proposals.jsonl ==="
if [ -f "$PROPOSALS" ]; then
  LINES=$(wc -l < "$PROPOSALS" | tr -d ' ')
  MTIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M:%S" "$PROPOSALS" 2>/dev/null || stat -c "%y" "$PROPOSALS" 2>/dev/null | cut -d. -f1)
  if [ "$LINES" -gt 0 ]; then
    ok "proposals.jsonl 存在，$LINES 行，mtime $MTIME"
    # 检查 timestamp/session 字段：两者都必须在（session_id 是回链会话的完整性关键字段，
    # 只要有其一就放行会漏掉「有 ts 没 sid」的半截 proposal）
    LAST_LINE=$(tail -1 "$PROPOSALS")
    HAS_TS=$(echo "$LAST_LINE" | grep -oE '"created_at_ms":[0-9]+' | head -1)
    HAS_SID=$(echo "$LAST_LINE" | grep -oE '"session_id":"[^"]+"' | head -1)
    if [ -n "$HAS_TS" ] && [ -n "$HAS_SID" ]; then
      ok "  最近一条带 timestamp/session: $HAS_TS $HAS_SID"
    else
      fail "  最近一条缺 timestamp 或 session（ts=$HAS_TS sid=${HAS_SID}）"
    fi
  else
    fail "proposals.jsonl 存在但 0 行（提取器没触发？）"
  fi
else
  fail "proposals.jsonl 不存在（环 2 断：提取器没写入）"
fi

echo ""
echo "=== ③ evolution-changes.jsonl（gate 通过） ==="
if [ -f "$CHANGES" ]; then
  LINES=$(wc -l < "$CHANGES" | tr -d ' ')
  MTIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M:%S" "$CHANGES" 2>/dev/null || stat -c "%y" "$CHANGES" 2>/dev/null | cut -d. -f1)
  if [ "$LINES" -gt 0 ]; then
    ok "changes.jsonl 存在，$LINES 行，mtime $MTIME"
  else
    fail "changes.jsonl 存在但 0 行（环 3 断：gate 全滤？）"
  fi
else
  fail "changes.jsonl 不存在（环 3 断：shadow 没写）"
fi

echo ""
echo "=== ④ evolution/ 目录 mtime ==="
if [ -d "$EVOLUTION_DIR" ]; then
  DIR_MTIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M:%S" "$EVOLUTION_DIR" 2>/dev/null || stat -c "%y" "$EVOLUTION_DIR" 2>/dev/null | cut -d. -f1)
  NEWEST_FILE=$(ls -t "$EVOLUTION_DIR" 2>/dev/null | head -1)
  if [ -n "$NEWEST_FILE" ]; then
    ok "evolution/ 目录 mtime ${DIR_MTIME}，最新文件：$NEWEST_FILE"
  else
    fail "evolution/ 目录存在但无文件"
  fi
else
  fail "evolution/ 目录不存在（环 4 断：落盘路径错）"
fi

echo ""
echo "=== ⑤ audit 日志（应有 no_policy_applied，不是 Allow） ==="
# 找最近的 audit 日志位置（老板本地：通常在 ~/Library/Logs/wmessage/ 或 app data dir）
# 简化：用 jq 找 system log 或本地 audit.jsonl
AUDIT_CANDIDATES=(
  "$HOME/Library/Application Support/wmessage/audit.jsonl"
  "$HOME/Library/Logs/wmessage/audit.log"
  "$EVOLUTION_DIR/audit.jsonl"
)
AUDIT_FOUND=""
for f in "${AUDIT_CANDIDATES[@]}"; do
  if [ -f "$f" ]; then
    AUDIT_FOUND="$f"
    break
  fi
done
if [ -z "$AUDIT_FOUND" ]; then
  # fallback: 抓 changes.jsonl 里的 audit 字段（如有）或 last audit_event
  echo "  ⚠ 未找到独立 audit 文件，检查 changes.jsonl 是否带 audit_event"
  if [ -f "$CHANGES" ] && grep -q "audit_event\|no_policy_applied" "$CHANGES"; then
    COUNT=$(grep -c "no_policy_applied" "$CHANGES" 2>/dev/null || echo 0)
    if [ "$COUNT" -gt 0 ]; then
      ok "changes.jsonl 含 $COUNT 条 no_policy_applied 审计"
    else
      fail "无 no_policy_applied 审计"
    fi
  else
    echo "  ⚠ 无法定位 audit 文件，跳过（老板自查 Tauri 应用日志）"
  fi
else
  LAST_AUDIT=$(tail -1 "$AUDIT_FOUND" 2>/dev/null)
  if echo "$LAST_AUDIT" | grep -q "no_policy_applied"; then
    ok "最近 audit = no_policy_applied（S0 正确态）"
  elif echo "$LAST_AUDIT" | grep -qE "allow|Allow"; then
    fail "最近 audit = Allow（误进 S2！状态跑偏）"
  elif echo "$LAST_AUDIT" | grep -qE "block|Block"; then
    fail "最近 audit = Block（S0 不该有，状态跑偏）"
  else
    echo "  ⚠ 最近 audit 不含已知决策类型：$(echo "$LAST_AUDIT" | head -c 100)"
  fi
fi

echo ""
echo "=== ⑥ 主记忆 mtime（应不变，§12.2 硬约束） ==="
# wmessage 主记忆数据库位置
DB_CANDIDATES=(
  "$EVOLUTION_DIR/wmessage.db"   # 便携模式：真库就是上面探到的 data dir，否则 §12.2 查错文件
  "$HOME/Library/Application Support/wmessage/wmessage.db"
  "$HOME/.wmessage/memories.json"
)
DB_FOUND=""
for db in "${DB_CANDIDATES[@]}"; do
  if [ -f "$db" ]; then
    DB_FOUND="$db"
    DB_MTIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M:%S" "$db" 2>/dev/null || stat -c "%y" "$db" 2>/dev/null | cut -d. -f1)
    ok "主记忆 $db mtime ${DB_MTIME}（与上一次对比；S0 下不应新写）"
  fi
done
if [ -z "$DB_FOUND" ]; then
  fail "主记忆库三候选全 miss（环 6 断：落盘路径错或库未创建）"
fi

echo ""
echo "=== ⑦ synthetic 隔离（不应混入真数据） ==="
if [ -d "$SYNTHETIC_DIR" ]; then
  SYNTH_SIZE=$(du -sh "$SYNTHETIC_DIR" 2>/dev/null | cut -f1)
  EVO_SIZE=$(du -sh "$EVOLUTION_DIR" 2>/dev/null | cut -f1)
  ok "synthetic 在 $SYNTHETIC_DIR ($SYNTH_SIZE)；真数据在 $EVOLUTION_DIR ($EVO_SIZE)"
else
  echo "  ⚠ $SYNTHETIC_DIR 不存在（隔离目录被删？）"
fi

echo ""
echo "================================"
echo "结果：$OK OK / $FAIL FAIL"
if [ "$FAIL" -eq 0 ]; then
  echo "✓ 全部断言通过，pipeline 正常运转"
  exit 0
else
  echo "✗ 有 $FAIL 项失败，按上方环号定位断点"
  exit 1
fi
