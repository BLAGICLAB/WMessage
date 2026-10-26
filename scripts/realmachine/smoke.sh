#!/usr/bin/env bash
# 实机冒烟脚本（macOS）：把 docs/realmachine-smoke-test-2026-10-24.md 的 14 项
# 冒烟里可脚本化的部分自动化，人工段以暂停提示衔接，输出与冒烟报告同构的
# PASS/FAIL 清单，可直接贴进报告。
#
# 用法：
#   bash scripts/realmachine/smoke.sh                 # 默认跑 src-tauri/target/debug/wmessage
#   bash scripts/realmachine/smoke.sh --app <exe>     # 指定构建产物（便携包也可）
#
# 依赖（缺一项对应检查项会 SKIP 并提示）：
#   - macOS 辅助功能 + 屏幕录制授权（osascript 合成键 / screencapture）
#   - sqlite3（只读连接查库：file:...?mode=ro，不碰应用状态）
#   - scripts/regression-probe.sh（水位 mark/report，同目录上级）
#
# 边界：只做冒烟——启动/静态面/退出清理；38 卡行为面与变异注入不在此脚本。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROBE="$ROOT/scripts/regression-probe.sh"
OUT_DIR="${TMPDIR:-/tmp}/smoke-$(date +%Y%m%d-%H%M%S)"
APP="$ROOT/src-tauri/target/debug/wmessage"

PASS=0; FAIL=0; SKIP=0; RESULTS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --app) APP="$2"; shift 2 ;;
    *) echo "未知参数：$1"; exit 2 ;;
  esac
done

result() { # result <PASS|FAIL|SKIP> <编号与名称> <证据一行>
  RESULTS+=("$1|$2|$3")
  case "$1" in
    PASS) PASS=$((PASS+1)); printf '  ✓ %-4s %s\n' "$2" "$3" ;;
    FAIL) FAIL=$((FAIL+1)); printf '  ✗ %-4s %s\n' "$2" "$3" ;;
    SKIP) SKIP=$((SKIP+1)); printf '  - %-4s %s\n' "$2" "$3" ;;
  esac
}

echo "══ wmessage 实机冒烟 ══"
echo "  产物：$APP"
echo "  输出：$OUT_DIR"
mkdir -p "$OUT_DIR"

[ -x "$APP" ] || { echo "✗ 找不到可执行产物：$APP（先 npm run tauri dev 或传 --app）"; exit 1; }
command -v sqlite3 >/dev/null || echo "⚠️ 无 sqlite3，库检查项将 SKIP"

# ── 阶段 A：启动前静态面 + 水位 ──────────────────────────────────────────
echo "── 阶段 A：启动与静态面（自动）──"

DATA_DIR="$("$PROBE" locate | sed -n 's/^data_dir * = //p' | head -1)"
if [ -z "$DATA_DIR" ]; then
  # locate 失败（应用从未跑过）时按 dev 兜底目录探
  DATA_DIR="$HOME/Library/Application Support/com.renshi.wmessage"
fi
BOT_LOG="$DATA_DIR/bot.log"
ERR_LOG="$DATA_DIR/error.log"
DB="$DATA_DIR/wmessage.db"

ERR_BEFORE=0
[ -f "$ERR_LOG" ] && ERR_BEFORE=$(wc -l <"$ERR_LOG" | tr -d ' ')
bash "$PROBE" mark >/dev/null 2>&1 \
  && result PASS "5" "探针 mark 水位落盘（report 收尾出）" \
  || result SKIP "5" "探针 mark 失败（bot.log 不存在？）"

if command -v sqlite3 >/dev/null && [ -f "$DB" ]; then
  RO="file:$DB?mode=ro"
  IC="$(sqlite3 "$RO" "PRAGMA integrity_check;" 2>/dev/null)"
  [ "$IC" = "ok" ] \
    && result PASS "4" "PRAGMA integrity_check = ok" \
    || result FAIL "4" "integrity_check = ${IC:-查询失败}"
  TABLES="$(sqlite3 "$RO" "SELECT count(*) FROM sqlite_master WHERE type='table';" 2>/dev/null)"
  [ "${TABLES:-0}" -ge 20 ] \
    && result PASS "4b" "关键表在位（$TABLES 张表）" \
    || result FAIL "4b" "表数量异常：${TABLES:-查不到}"
  # subagents usage 三列（历史冒烟的迁移列检查：表在则列必须在）
  if sqlite3 "$RO" "SELECT 1 FROM sqlite_master WHERE type='table' AND name='subagents';" 2>/dev/null | grep -q 1; then
    COLS="$(sqlite3 "$RO" "PRAGMA table_info(subagents);" 2>/dev/null)"
    MISSING=""
    for c in prompt_tokens completion_tokens cache_read_tokens; do
      echo "$COLS" | grep -q "|$c|" || MISSING="$MISSING $c"
    done
    [ -z "$MISSING" ] \
      && result PASS "3" "subagents usage 三列在位" \
      || result FAIL "3" "subagents 缺列：$MISSING"
  else
    result SKIP "3" "无 subagents 表（全新库，迁移列无从谈起）"
  fi
else
  result SKIP "4" "无 sqlite3 或无库文件，跳过库检查"
fi

# ── 阶段 B：启动 ─────────────────────────────────────────────────────────
echo "── 阶段 B：启动等待（自动）──"
STDERR_LOG="$OUT_DIR/stderr.log"
"$APP" >"$OUT_DIR/stdout.log" 2>"$STDERR_LOG" &
APP_PID=$!
BOOT_OK=0
for i in $(seq 1 30); do
  sleep 1
  kill -0 "$APP_PID" 2>/dev/null || break
  if grep -qiE 'panic|thread .* panicked' "$STDERR_LOG"; then break; fi
  if [ $i -ge 12 ]; then BOOT_OK=1; break; fi
done
if [ "$BOOT_OK" = 1 ]; then
  result PASS "1" "启动 12s 无 panic、进程存活"
else
  result FAIL "1" "启动失败/panic，看 $STDERR_LOG"
fi

sleep 3
screencapture -x "$OUT_DIR/smoke-boot.png" 2>/dev/null \
  && result PASS "6a" "启动截图 → $OUT_DIR/smoke-boot.png" \
  || result SKIP "6a" "screencapture 失败（屏幕录制授权？）"

# ── 阶段 C：人工段（界面 + 两轮往返）────────────────────────────────────
echo "── 阶段 C：人工段（按提示操作，回车继续）──"
echo "  1) 依次切到 首页/任务/聊天/设置 页，每切一页回来按一次回车（脚本截图）"
for page in main tasks chat settings; do
  read -r -p "  已切到 [$page] 页？回车截图..." _
  screencapture -x "$OUT_DIR/smoke-$page.png" 2>/dev/null && echo "    → smoke-$page.png"
done
echo "  2) 在聊天页发两轮消息（第一轮：请只回复「冒烟通过」；第二轮随便应一句）"
read -r -p "  两轮都回复完成？回车继续..." _
# 等两轮 trace.complete（水位后新出现 ≥2 条）
COMPLETE=0
if [ -f "$BOT_LOG" ]; then
  SLICE_START=$(sed -n '2p' "${TMPDIR:-/tmp}/wmessage-regression-probe/mark" 2>/dev/null)
  if [ -n "$SLICE_START" ] && [ "$SLICE_START" -gt 0 ] 2>/dev/null; then
    for i in $(seq 1 30); do
      COMPLETE=$(tail -c +"$((SLICE_START + 1))" "$BOT_LOG" 2>/dev/null | grep -c 'trace.complete' || true)
      [ "${COMPLETE:-0}" -ge 2 ] && break
      sleep 2
    done
  fi
fi
# ${COMPLETE} 必须加大括号：紧随其后的 ×/（ 是多字节字符，老版 bash 解析
# $COMPLETE 时会把这些字节吞进变量名 → unbound variable 直接死在 C→D 接缝
[ "${COMPLETE:-0}" -ge 2 ] \
  && result PASS "10/11" "两轮往返 trace.complete ×${COMPLETE}（审计链完整）" \
  || result SKIP "10/11" "水位后未见 2 条 trace.complete（${COMPLETE} 条）——人工核对回复与用量条"

# ── 阶段 D：优雅退出 + 收尾 ─────────────────────────────────────────────
echo "── 阶段 D：优雅退出与收尾（自动）──"
osascript -e 'tell application "System Events" to keystroke "q" using command down' 2>/dev/null
EXIT_OK=0
for i in $(seq 1 15); do
  sleep 1
  kill -0 "$APP_PID" 2>/dev/null || { EXIT_OK=1; break; }
done
if [ "$EXIT_OK" = 1 ]; then
  result PASS "13" "Cmd+Q 后 15s 内进程退出"
  if [ -f "$BOT_LOG" ] && tail -c 8192 "$BOT_LOG" | grep -q 'app_exit_cleanup'; then
    result PASS "13b" "退出清理链事件在（app_exit_cleanup）"
  else
    result SKIP "13b" "近 8KB 未抓到 app_exit_cleanup（日志轮转/慢写？人工查）"
  fi
else
  result FAIL "13" "Cmd+Q 未退出（辅助功能授权？），手动退出后继续"
  kill "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
fi

ERR_AFTER=0
[ -f "$ERR_LOG" ] && ERR_AFTER=$(wc -l <"$ERR_LOG" | tr -d ' ')
[ "$ERR_AFTER" -le "$ERR_BEFORE" ] \
  && result PASS "2" "error.log 零新增（$ERR_BEFORE → $ERR_AFTER 行）" \
  || result FAIL "2" "error.log 新增 $((ERR_AFTER - ERR_BEFORE)) 行，看 $ERR_LOG"

echo "── 事件区间（探针 report）──"
bash "$PROBE" report 2>/dev/null | head -30 || result SKIP "5b" "probe report 失败"

echo
echo "══ 汇总：PASS=$PASS  FAIL=$FAIL  SKIP=$SKIP ══"
echo "  截图与日志：$OUT_DIR"
if [ $FAIL -gt 0 ]; then
  echo "  ✗ 有 FAIL 项，逐条看上面证据行"
  exit 1
fi
echo "  ✓ 自动项全过；SKIP 项按提示人工核对后贴进冒烟报告"
