#!/usr/bin/env bash
# 溢出回灌的真机地面真相读取器。
#
# GUI 里看到的只是「模型收到的那一截」，这个脚本看的是模型**没**看到的东西：
# 转存出去的完整文件、bot.log 里的处置记录、保留期清扫的结果。
#
# 用法：
#   overflow-probe.sh locate    # 打印 data_dir / bot.log / 溢出目录，并确认找对了
#   overflow-probe.sh mark      # 记水位线（后续 report 只报水位之后的新事件）
#   overflow-probe.sh report    # 汇总水位之后的溢出事件 + 溢出文件清单
#   overflow-probe.sh watch     # 实时跟流水（另开一个终端跑）
#   overflow-probe.sh show <f>  # 打印某溢出文件的规模 + 头尾各一截
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MARK_FILE="$ROOT/.overflow-probe.mark"

# bot.log 落在 data_dir。dev 构建的 exe 在 target/debug → 便携分支把 data_dir
# 定在 exe 同目录；打包后 exe 可能在任意可写目录，或退化到系统 app_data。
# 两类都探一遍，让人自己确认命中的是哪个（探错目录 = 后面全白测）。
candidates=(
  "$ROOT/src-tauri/target/debug"
  "$HOME/Library/Application Support/com.renshi.wmessage"
  "$HOME/Library/Application Support/wmessage"
  "$HOME/AppData/Roaming/com.renshi.wmessage"
  "$ROOT/src-tauri/target/release"
)

# 返回 bot.log 的完整路径（不是它所在的目录——调用方要 dirname 一次才是 data_dir）
find_bot_log() {
  local c
  for c in "${candidates[@]}"; do
    [ -f "$c/bot.log" ] && { printf '%s\n' "$c/bot.log"; return 0; }
  done
  return 1
}

dir_of() {
  local log; log="$(find_bot_log)" || return 1
  printf '%s\n' "$(dirname "$log")"
}

cmd_locate() {
  local log
  if ! log="$(find_bot_log)"; then
    echo "✗ 没找到 bot.log。应用跑过吗？探过的目录："
    printf '    %s\n' "${candidates[@]}"
    return 1
  fi
  local d; d="$(dirname "$log")"
  echo "data_dir   = $d"
  echo "bot.log    = $log  ($(wc -c <"$log" | tr -d ' ') 字节)"
  local ov="$d/overflow"
  if [ -d "$ov" ]; then
    echo "溢出目录    = $ov"
    echo "  现有溢出文件 $(find "$ov" -type f | wc -l | tr -d ' ') 个"
  else
    echo "溢出目录    = $ov  （尚不存在——说明这次运行还没发生过溢出，或版本早于该功能）"
  fi
  # 水位线是相对字节偏移量的，bot.log 会轮转；水位之后若发生轮转要作废重来
  if [ -f "$MARK_FILE" ]; then
    local mk_log mk_off
    mk_log=$(sed -n '1p' "$MARK_FILE"); mk_off=$(sed -n '2p' "$MARK_FILE")
    if [ "$mk_log" != "$log" ]; then
      echo "⚠️  水位线记在另一个 bot.log 上（$mk_log），请重新 mark"
    else
      local cur; cur=$(wc -c <"$log" | tr -d ' ')
      if [ "$cur" -lt "$mk_off" ]; then
        echo "⚠️  bot.log 已轮转（当前 $cur < 水位 $mk_off），水位失效，请重新 mark"
      fi
    fi
  fi
}

cmd_mark() {
  local log; log="$(find_bot_log)" || { echo "✗ 没找到 bot.log"; return 1; }
  local off; off=$(wc -c <"$log" | tr -d ' ')
  printf '%s\n%s\n' "$log" "$off" > "$MARK_FILE"
  echo "✓ 水位线：$log @ $off 字节"
  echo "  现在去 GUI 跑场景，回来执行：overflow-probe.sh report"
}

# 从水位之后抽出与溢出漏斗相关的三类审计：
#   tool.output.over_budget  — 漏斗介入前，原始长度（dispatch 层）
#   tool.output.spilled      — 处置结果：转存成功，附路径
#   tool.output.spill_failed — 转存失败，退回无文件截断
cmd_report() {
  local log; log="$(find_bot_log)" || { echo "✗ 没找到 bot.log"; return 1; }
  local off=0
  [ -f "$MARK_FILE" ] && off=$(sed -n '2p' "$MARK_FILE")
  local slice; slice=$(tail -c "+$((off+1))" "$log")

  echo "── 处置事件（bot.log 字节 ${off}+）──"
  local ev
  ev=$(printf '%s' "$slice" | grep -E 'tool\.output\.(over_budget|spilled|spill_failed)' || true)
  if [ -z "$ev" ]; then
    echo "  （无）"
  else
    printf '%s\n' "$ev" | sed 's/^/  /'
  fi

  echo
  echo "── 计数 ──"
  printf '  over_budget  %s\n' "$(printf '%s' "$ev" | grep -c 'over_budget' || true)"
  printf '  spilled      %s\n' "$(printf '%s' "$ev" | grep -c 'tool\.output\.spilled' || true)"
  printf '  spill_failed %s\n' "$(printf '%s' "$ev" | grep -c 'spill_failed' || true)"

  # 工具调用流水：A1/A2 不超 cap，日志里连 over_budget 都不会有——「什么都没发生」
  # 和「模型压根没跑工具」在处置事件里长得一模一样。这段给出「跑过了」的凭证：
  # tool.return 每次工具调用都记，带 preview（结果前 80 字符）。
  echo
  echo "── 工具调用流水（证明模型确实动手了）──"
  local runs
  runs=$(printf '%s' "$slice" | grep -E '\| tool\.return \|' || true)
  if [ -z "$runs" ]; then
    echo "  （无）——⚠️ 模型这一轮可能压根没调工具，别把「没溢出」当成通过"
  else
    printf '%s\n' "$runs" | sed -E 's/.*\| tool\.return \| //; s/ \| session_id=.*//' | sed 's/^/  /'
  fi

  # 幻觉守卫：模型声称跑过某工具但凭证池里没有时记一条。场景 H（模型会不会
  # 自己去读指针）里它就是现成的测谎仪。
  echo
  echo "── 幻觉守卫（模型声称执行但无凭证 = FAIL）──"
  local hg
  hg=$(printf '%s' "$slice" | grep -a 'hallucination_guard' || true)
  if [ -z "$hg" ]; then
    echo "  （无）"
  else
    printf '%s\n' "$hg" | sed 's/^/  /'
  fi

  local ov; ov="$(dir_of)/overflow"
  echo
  echo "── 溢出文件清单 ──"
  if [ ! -d "$ov" ]; then
    echo "  （溢出目录不存在 = 一次都没转存过）"
    return 0
  fi
  # 按 mtime 倒序：最近的在前，用户刚跑的场景排在头几行
  find "$ov" -type f -name '*.txt' -exec stat -f '%m %z %N' {} \; 2>/dev/null \
    | sort -rn | head -20 | while read -r m sz f; do
        printf '  %s  %8s 字节  %s\n' "$(date -r "$m" '+%m-%d %H:%M:%S')" "$sz" "$f"
      done
  local n; n=$(find "$ov" -type f -name '*.txt' | wc -l | tr -d ' ')
  echo "  合计 $n 个（最多列 20）"
}

cmd_watch() {
  local log; log="$(find_bot_log)" || { echo "✗ 没找到 bot.log"; return 1; }
  echo "跟随 $log （Ctrl-C 停）"
  tail -f "$log" | grep --line-buffered -aE \
    'tool\.output\.|tool\.return|hallucination_guard|trace\.span_overflow|trace\.batch_dropped|overflow\.sweep_fail'
}

cmd_show() {
  local f="${1:-}"
  [ -n "$f" ] || { echo "用法：overflow-probe.sh show <溢出文件路径>"; return 1; }
  [ -f "$f" ] || { echo "✗ 文件不存在：$f"; return 1; }
  echo "路径   $f"
  echo "行数   $(wc -l <"$f" | tr -d ' ')   字节 $(wc -c <"$f" | tr -d ' ')"
  echo "字符数 $(python3 -c 'import sys;print(len(open(sys.argv[1],encoding="utf-8",errors="replace").read()))' "$f")"
  echo "── 头 200 字符 ──"
  head -c 400 "$f" | python3 -c 'import sys;print(sys.stdin.read()[:200])'
  echo
  echo "── 尾 200 字符 ──"
  tail -c 400 "$f" | python3 -c 'import sys;print(sys.stdin.read()[-200:])'
}

case "${1:-}" in
  locate) cmd_locate ;;
  mark)   cmd_mark ;;
  report) cmd_report ;;
  watch)  cmd_watch ;;
  show)   shift; cmd_show "$@" ;;
  *) echo "用法：$0 {locate|mark|report|watch|show <file>}"; exit 1 ;;
esac
