#!/usr/bin/env bash
# 真机回归探针：读本机日志与进程状态，作为实机验收的观测入口。
#
# 配套文档：docs/realmachine-regression-test.md
#
# Windows 用户用同名子命令的 PowerShell 版：scripts/regression-probe.ps1
#
# 子命令：
#   locate    打印 data_dir / bot.log / error.log / 技能目录 / 服务日志根目录
#   mark      在 bot.log 上打水位线（后续 report 只看这之后的行）
#   report    读水位线之后的审计事件，按域分组
#   services  列受管服务进程 + 监听端口（真机地面真相，不看应用自报）
#   orphans   查「宿主已不在、服务还活着」的孤儿
#   keyring   查 MCP 服务器的 keyring 机密条目是否还在
#   usage     自检
#
# 只读，不改任何应用状态。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

MARK_DIR="${TMPDIR:-/tmp}/wmessage-regression-probe"
MARK_FILE="$MARK_DIR/mark"

# ── 定位 ────────────────────────────────────────────────────────────────
# bot.log 落在 data_dir；dev 构建在 target/debug，便携包在 exe 同目录。
find_data_dir() {
  local d
  for d in \
    "$HOME/Library/Application Support/com.renshi.wmessage" \
    "$ROOT/src-tauri/target/debug" \
    "$ROOT/src-tauri/target/release" \
    "$ROOT"; do
    [ -f "$d/bot.log" ] && { printf '%s\n' "$d"; return 0; }
  done
  # 便携包：exe 同目录
  local exe
  exe="$(ls -1 "$ROOT"/src-tauri/target/release/wmessage 2>/dev/null | head -1)"
  [ -n "$exe" ] && [ -f "$(dirname "$exe")/bot.log" ] && { dirname "$exe"; return 0; }
  return 1
}

find_bot_log() {
  local d
  d="$(find_data_dir)" || return 1
  [ -f "$d/bot.log" ] && { printf '%s\n' "$d/bot.log"; return 0; }
  return 1
}

cmd_locate() {
  local d log
  if ! d="$(find_data_dir)"; then
    echo "✗ 没找到 data_dir。应用跑过吗？探过的目录："
    echo "  $HOME/Library/Application Support/com.renshi.wmessage"
    echo "  $ROOT/src-tauri/target/{debug,release}"
    return 1
  fi
  log="$d/bot.log"
  echo "data_dir      = $d"
  echo "bot.log       = $log"
  if [ -f "$log" ]; then
    echo "  大小         = $(wc -c <"$log" | tr -d ' ') 字节"
  else
    echo "  （还没有日志，跑一次 GUI 再回来）"
  fi
  echo "error.log     = $d/error.log$( [ -f "$d/error.log" ] && echo '（WARN/ERROR 镜像）' || echo '（暂无）' )"
  # 技能刻意住在 app_data_dir 而非 data_dir：日志/db 跟便携策略（exe 同目录），
  # 已安装的技能留在 app_data，避免被便携策略一起搬走。
  # 见 bot_skills/manage.rs::skills_dir。
  case "$(uname -s)" in
    Darwin) APP_DATA="$HOME/Library/Application Support/com.renshi.wmessage" ;;
    *)      APP_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/com.renshi.wmessage" ;;
  esac
  echo "技能目录      = $APP_DATA/skills（app_data，与 data_dir 解耦）"
  echo "内置技能目录  = $APP_DATA/builtin_skills"
  echo "配置文件      = $d/bot-config.json"
  echo "服务日志根    = ${TMPDIR:-/tmp}/wmessage-services"
  echo "flag 目录     = $d/runtime/flags"
  return 0
}

# ── 水位线 ──────────────────────────────────────────────────────────────
cmd_mark() {
  local log off
  log="$(find_bot_log)" || { echo "✗ 没找到 bot.log"; return 1; }
  mkdir -p "$MARK_DIR"
  off="$(wc -c <"$log" | tr -d ' ')"
  printf '%s\n%s\n' "$log" "$off" >"$MARK_FILE"
  echo "✓ 水位线已记：$log @ $off"
}

read_mark() {
  [ -f "$MARK_FILE" ] || { echo ""; return 1; }
  local l o
  l="$(sed -n '1p' "$MARK_FILE")"
  o="$(sed -n '2p' "$MARK_FILE")"
  [ -f "$l" ] || return 1
  local cur
  cur="$(wc -c <"$l" | tr -d ' ')"
  if [ "$cur" -lt "$o" ]; then
    echo "⚠️  bot.log 已轮转（水位 $o > 当前 $cur），水位失效，请重新 mark" >&2
    return 1
  fi
  printf '%s\n' "$l"
  return 0
}

# ── 事件报告 ────────────────────────────────────────────────────────────
cmd_report() {
  local log off slice
  log="$(read_mark)" || { echo "✗ 没有有效水位线，先跑 mark"; return 1; }
  off="$(sed -n '2p' "$MARK_FILE")"
  slice="$MARK_DIR/slice"
  mkdir -p "$MARK_DIR"
  tail -c "+$((off + 1))" "$log" >"$slice"
  echo "── 事件（bot.log 字节 ${off}+）──"
  # 事件名有两种落法：带级别的「[ts] LEVEL | 事件 | kv」与无级别的「[ts] 事件 | kv」。
  # 两种都要匹配上——只认「 | 」会把 mcp.builtin_materialized 这类整条漏掉。
  grep -E '([|] |\] )(service|mcp|skill|subagent|tool|pre_step|app_exit|bot_fs|reminder|ui|manage_file|desktop|confirm|loop|final_selfcheck|memory)\.' \
    "$slice" || echo "（本区间没有匹配事件）"
  echo
  echo "── 自由格式技能行 ──"
  # 锚点必须容下时间戳前缀：日志行以「[时间戳] 」开头，行首锚定永远匹配不到。
  grep -E '(^|\] )(skill_|confirm|skill_dsl_)' "$slice" || echo "（无）"
  echo
  echo "── 工具调用与失败 ──"
  grep -E 'tool\.(call|return|panic)|工具输出超预算|超预算|budget_tool_calls_exceeded' "$slice" \
    | tail -40 || echo "（无）"
}

# ── 受管服务：真机地面真相 ───────────────────────────────────────────────
# pid 的可靠来源是 bot.log 的 service.started 事件（kv 里带 pid=）——
# stdout.log 里只有服务的输出，pid 不在那儿。
service_pids_from_log() {
  local log
  log="$(find_bot_log)" || return 1
  grep -a 'service\.started' "$log" | grep -oE 'service_id=[^ |]+|pid=[0-9]+' \
    | paste - - 2>/dev/null | sed 's/service_id=//; s/pid=//'
}

cmd_services() {
  local root="${TMPDIR:-/tmp}/wmessage-services"
  root="${root%/}"
  echo "── 服务日志根 $root ──"
  local pairs found=0
  pairs="$(service_pids_from_log)"
  if [ -z "$pairs" ]; then
    echo "bot.log 里没有 service.started 事件——这次运行还没起过受管服务。"
  else
    while IFS=$'\t' read -r sid pid; do
      [ -n "${pid:-}" ] || continue
      found=1
      echo "── $sid  pid=$pid ──"
      if ps -p "$pid" >/dev/null 2>&1; then
        ps -o pid=,ppid=,pgid=,stat=,command= -p "$pid" | sed 's/^/  /'
        echo "  监听："
        lsof -nP -a -p "$pid" -iTCP -sTCP:LISTEN 2>/dev/null | tail -n +2 | sed 's/^/    /'
        lsof -nP -a -p "$pid" -iUDP 2>/dev/null | tail -n +2 | sed 's/^/    udp| /'
      else
        echo "  进程 $pid 已不在（已退出 / 已被停 / 随宿主一起被杀）"
      fi
    done <<<"$pairs"
  fi
  [ "$found" -eq 0 ] && return 0

  echo
  echo "── 各服务输出尾部 ──"
  for svc in "$root"/*/; do
    [ -d "$svc" ] || continue
    echo "── $(basename "$svc") ──"
    tail -3 "$svc/stdout.log" 2>/dev/null | sed 's/^/  out| /'
    tail -3 "$svc/stderr.log" 2>/dev/null | sed 's/^/  err| /'
  done
}

# ── 孤儿：宿主不在但服务还活着 ──────────────────────────────────────────
cmd_orphans() {
  local root="${TMPDIR:-/tmp}/wmessage-services"
  root="${root%/}"
  echo "── 孤儿检查 ──"
  local app_pids
  app_pids="$(pgrep -f 'wmessage' 2>/dev/null | tr '\n' ' ')"
  echo "宿主进程 PID：${app_pids:-（没找到在跑的 wmessage）}"
  local pairs found=0
  pairs="$(service_pids_from_log)"
  if [ -n "$pairs" ]; then
    while IFS=$'\t' read -r sid pid; do
      [ -n "${pid:-}" ] || continue
      if ps -p "$pid" >/dev/null 2>&1; then
        found=1
        echo "  ⚠️  $sid 的 pid=$pid 仍存活"
        ps -o pid=,ppid=,command= -p "$pid" | sed 's/^/       /'
      fi
    done <<<"$pairs"
  fi
  [ "$found" -eq 0 ] && echo "  ✓ 没有孤儿服务进程"
  echo "  （提示：宿主崩溃后看门狗收组可能有几秒延迟，等 5~10s 再查一次）"
  return 0
}

# ── keyring ─────────────────────────────────────────────────────────────
cmd_keyring() {
  echo "── MCP keyring 条目（service = wmessage.bot.v1）──"
  if ! command -v security >/dev/null 2>&1; then
    echo "（非 macOS：本机无 keychain CLI。Linux 无 secret-service 时降级为 bot-mcp-secrets.json）"
    return 0
  fi
  security dump-keychain 2>/dev/null | grep -B1 -A2 'mcp:' | sed 's/^/  /' || echo "  （没找到 mcp: 开头的条目）"
  echo
  echo "  按服务器逐个查：security find-generic-password -s \"wmessage.bot.v1\" -a \"mcp:<id>\" -w"
}

# ── 本轮修复的证据 ──────────────────────────────────────────────────────
# 与 Windows 侧 regression-probe.ps1 的 fixes 子命令同义：
# 一次命令拿到「这轮修的东西在日志里的落点」，避免在几千行 bot.log 里翻。
cmd_fixes() {
  local log
  log="$(find_bot_log)" || { echo "✗ 没找到 bot.log（先跑 locate）"; return 1; }

  echo "── 修复复验证据 ──"
  echo "  bot.log = $log"
  echo

  echo "── D-2：open_url / fetch_url 的失败级别（同因必须同级）──"
  local tool_lines
  tool_lines="$(grep -aE 'tool\.return .*tool=(open_url|fetch_url)' "$log" | tail -8)"
  if [ -n "$tool_lines" ]; then
    printf '%s\n' "$tool_lines" | sed 's/^/  /'
    echo "  期望：DNS 解析失败在 open_url 与 fetch_url 上都是 ERROR；策略拒绝仍是 WARN"
  else
    echo "  （还没有 open_url / fetch_url 的 tool.return 行）"
  fi
  echo

  echo "── D-6：就绪超时的归因 ──"
  local svc_lines
  svc_lines="$(grep -aE 'service\.(started|ready|stop_requested|exited|stopped|start_failed)' "$log" | tail -15)"
  if [ -n "$svc_lines" ]; then
    printf '%s\n' "$svc_lines" | sed 's/^/  /'
    echo "  期望：start_failed 的 reason 是「在 Ns 内未就绪」，绝不是「启动后立即退出」"
    echo "  期望：service.exited 那行带 stop_reason=ready_timeout"
  else
    echo "  （还没有服务生命周期行）"
  fi
  echo

  echo "── D-1：构建机路径泄漏（必须为空）──"
  # 判据不是「文本里长得像盘符」（Python 代码里的 as e:\n 会误报），
  # 而是地面真相：mcp.builtin_materialized 记的包路径在本机**存不存在**。
  local mats leaks=""
  mats="$(grep -a 'mcp\.builtin_materialized' "$log" | tail -3)"
  if [ -n "$mats" ]; then
    printf '%s\n' "$mats" | sed 's/^/  /'
    local p
    while IFS= read -r p; do
      [ -n "$p" ] || continue
      if [ ! -e "$p" ]; then
        leaks="  [!] 路径在本机不存在：$p
         （要么包没随包分发，要么这个路径是别的机器留下的——对照上一行应用自己写的状态）
"
      fi
    done <<<"$(printf '%s\n' "$mats" | sed -n 's/.*包: \(.*\)（.*/\1/p')"
    if [ -n "$leaks" ]; then
      printf '%s' "$leaks"
    else
      echo "  ✓ 内置包路径在本机存在，没有构建机路径泄漏"
    fi
  else
    echo "  （还没有 mcp.builtin_materialized 事件）"
  fi
  echo

  echo "── 人眼项（面板是 WebView，脚本读不到 DOM）──"
  echo "  D-7：本地服务面板必须出现能力提示，且不得出现「合计内存 0 MB」"
  echo "  D-8：截屏不得弹出控制台窗口（macOS 上无 Windows 那个坑，看不到就跳过）"
}

cmd_usage() {
  cat <<'EOF'
用法：bash scripts/regression-probe.sh <子命令>

  locate    打印 data_dir / bot.log / 技能目录 / 服务日志根
  mark      在 bot.log 上打水位线
  report    读水位线之后的审计事件（按 service/mcp/skill/subagent/tool 分组）
  services  列受管服务的真实进程与监听端口
  orphans   查宿主已死、服务还活着的孤儿
  keyring   查 MCP 服务器的 keyring 机密条目
  fixes     打印本轮修好的缺陷的日志证据（D-1 / D-2 / D-6）
  usage     本帮助
EOF
}

case "${1:-usage}" in
  locate)   cmd_locate ;;
  mark)     cmd_mark ;;
  report)   cmd_report ;;
  services) cmd_services ;;
  orphans)  cmd_orphans ;;
  keyring)  cmd_keyring ;;
  fixes)    cmd_fixes ;;
  usage)    cmd_usage ;;
  *)        cmd_usage; exit 1 ;;
esac
