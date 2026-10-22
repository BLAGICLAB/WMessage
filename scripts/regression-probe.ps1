# Real-machine regression probe (Windows edition).
# Reads local logs / process state; read-only, never mutates app state.
#
# Companion doc: docs/realmachine-regression-test.md
#
# This file is intentionally ASCII-only. PowerShell 5.1 decodes .ps1 files
# using the system ANSI code page (GBK on zh-CN Windows) unless the file
# carries a UTF-8 BOM. Non-ASCII bytes then become mojibake, and a mangled
# multi-byte sequence can swallow a quote and break string literals -- a
# parse error that looks like a syntax bug but is really an encoding bug.
# Keeping the source ASCII removes the whole failure class.
#
# Subcommands:
#   locate    resolve data_dir (portable dir vs %APPDATA% fallback) / bot.log / service log root
#   mark      stamp a byte watermark on bot.log
#   report    dump audit events recorded after the watermark
#   services  list managed-service processes and their listening ports
#   orphans   find services still alive after the host is gone
#   keyring   list MCP credential-manager entries
#   fixes     dump the log evidence for the fixed defects (D-1 / D-2 / D-6)
#   usage     self help
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File regression-probe.ps1 locate
#   powershell -ExecutionPolicy Bypass -File regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-22"
[CmdletBinding()]
param(
  [Parameter(Position = 0)]
  [string]$Command = 'usage',

  # Portable package dir (the one holding wmessage.exe). Auto-detected if omitted.
  [string]$ExeDir = ''
)

$ErrorActionPreference = 'Continue'
$MarkDir = Join-Path $env:TEMP 'wmessage-regression-probe'
$MarkFile = Join-Path $MarkDir 'mark.json'

# ---------------------------------------------------------------- locating

function Resolve-ExeDir {
  param([string]$Hint)
  if ($Hint -and (Test-Path (Join-Path $Hint 'wmessage.exe'))) { return (Resolve-Path $Hint).Path }
  if ($Hint) { return $Hint }

  if (Test-Path (Join-Path (Get-Location).Path 'wmessage.exe')) { return (Get-Location).Path }

  # Common spots: portable packages are often dropped in a drive root or a Program Files dir.
  $roots = @(
    'D:\', 'C:\',
    'D:\Program Files', 'C:\Program Files', 'D:\Program Files (x86)',
    (Join-Path $env:USERPROFILE 'Desktop'), (Join-Path $env:USERPROFILE 'Downloads')
  )
  foreach ($root in $roots) {
    if (-not (Test-Path $root)) { continue }
    $hit = Get-ChildItem -Path $root -Directory -Filter 'wmessage-portable*' -ErrorAction SilentlyContinue |
      Select-Object -First 1
    if ($hit) { return $hit.FullName }
  }
  return ''
}

function Get-DataDirCandidates {
  param([string]$Dir)
  $cands = New-Object System.Collections.Generic.List[string]
  if ($Dir) { $cands.Add($Dir) }
  $cands.Add((Join-Path $env:APPDATA 'com.renshi.wmessage'))
  $cands.Add((Join-Path $env:LOCALAPPDATA 'com.renshi.wmessage'))
  return $cands
}

# The live data_dir is the one whose bot.log was written most recently.
function Resolve-DataDir {
  param([string]$Dir)
  $best = $null
  $bestTime = [datetime]::MinValue
  foreach ($c in (Get-DataDirCandidates $Dir)) {
    $log = Join-Path $c 'bot.log'
    if (-not (Test-Path $log)) { continue }
    $t = (Get-Item $log).LastWriteTime
    if ($t -gt $bestTime) { $bestTime = $t; $best = $c }
  }
  if ($best) { return $best }
  if ($Dir) { return $Dir }
  return (Join-Path $env:APPDATA 'com.renshi.wmessage')
}

function Get-BotLogPath {
  param([string]$Dir)
  $d = Resolve-DataDir $Dir
  return (Join-Path $d 'bot.log')
}

function Get-ServicesRoot {
  # Rust side uses std::env::temp_dir(); on Windows that is %TEMP%.
  return (Join-Path $env:TEMP 'wmessage-services')
}

function Cmd-Locate {
  param([string]$Dir)
  $exe = Resolve-ExeDir $Dir
  $data = Resolve-DataDir $Dir
  $log = Join-Path $data 'bot.log'

  Write-Host "exe dir           = $(if($exe){$exe}else{'(not found - pass -ExeDir)'})"
  Write-Host "data_dir          = $data"

  if (Test-Path $log) {
    $f = Get-Item $log
    Write-Host "bot.log           = $log"
    Write-Host "  size / written  = $($f.Length) bytes / $($f.LastWriteTime)"
  } else {
    Write-Host "bot.log           = $log (no log yet - has the app run?)"
  }

  $err = Join-Path $data 'error.log'
  Write-Host "error.log         = $err$(if(Test-Path $err){' (WARN/ERROR mirror)'}else{' (none)'})"

  # Skills deliberately live in app_data_dir, NOT in data_dir: logs/db follow
  # the portable strategy (next to the exe) while installed skills stay in
  # %APPDATA% so they are not dragged along by the portable policy.
  # See bot_skills/manage.rs::skills_dir.
  $appData = Join-Path $env:APPDATA 'com.renshi.wmessage'
  Write-Host "skills dir        = $(Join-Path $appData 'skills')   (app_data, decoupled from data_dir)"
  Write-Host "builtin skills    = $(Join-Path $appData 'builtin_skills')"
  Write-Host "config file       = $(Join-Path $data 'bot-config.json')"
  Write-Host "service log root  = $(Get-ServicesRoot)"
  Write-Host "credential store  = Windows Credential Manager (target name: mcp:<server id>)"

  # A non-writable exe dir makes the app fall back to %APPDATA%; the log records it.
  if (Test-Path $log) {
    $fallback = Select-String -Path $log -Pattern 'data_dir_fallback' -SimpleMatch -ErrorAction SilentlyContinue |
      Select-Object -Last 1
    if ($fallback) {
      Write-Host ''
      Write-Host '[!] data_dir fallback happened (last occurrence):' -ForegroundColor Yellow
      Write-Host "    $($fallback.Line)"
      Write-Host '    -> exe dir is not writable (Program Files needs admin rights, or AV lock).' -ForegroundColor Yellow
      Write-Host '    -> data lives under %APPDATA%, NOT next to the exe. Record this in the report.' -ForegroundColor Yellow
    }
  }

  $anchor = Join-Path $data 'wmessage.db'
  Write-Host ''
  Write-Host "portable anchor   = wmessage.db $(if(Test-Path $anchor){'present (anchored to data_dir)'}else{'absent (created on first run)'})"
}

# -------------------------------------------------------------- watermark

function Save-Mark {
  param([string]$Log)
  New-Item -ItemType Directory -Force -Path $MarkDir | Out-Null
  $off = (Get-Item $Log).Length
  @{ log = $Log; offset = $off } | ConvertTo-Json | Set-Content -Path $MarkFile -Encoding UTF8
  Write-Host "[ok] watermark set: $Log @ $off"
}

function Get-Mark {
  if (-not (Test-Path $MarkFile)) { return $null }
  $m = Get-Content $MarkFile -Raw -Encoding UTF8 | ConvertFrom-Json
  if (-not (Test-Path $m.log)) { return $null }
  $cur = (Get-Item $m.log).Length
  if ($cur -lt $m.offset) {
    Write-Host "[!] bot.log rotated (watermark $($m.offset) > current $cur) - watermark is void, run mark again" -ForegroundColor Yellow
    return $null
  }
  return $m
}

# Read the log from a byte offset. StreamReader with an explicit UTF8 encoding
# avoids the ANSI-codepage guess, so Chinese log lines come out readable.
function Get-TailFrom {
  param([string]$Log, [long]$Offset)
  $fs = [System.IO.File]::Open($Log, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  try {
    $null = $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $sr = New-Object System.IO.StreamReader($fs, (New-Object System.Text.UTF8Encoding($false)))
    return $sr.ReadToEnd()
  } finally {
    if ($fs) { $fs.Close() }
  }
}

function Cmd-Mark {
  param([string]$Dir)
  $log = Get-BotLogPath $Dir
  if (-not (Test-Path $log)) { Write-Host "[x] no bot.log at $log"; return }
  Save-Mark $log
}

function Cmd-Report {
  param([string]$Dir)
  $m = Get-Mark
  if (-not $m) { Write-Host '[x] no valid watermark - run mark first'; return }
  $text = Get-TailFrom $m.log $m.offset
  $lines = $text -split "`r?`n"

  Write-Host "-- events (bot.log byte $($m.offset)+) --"
  $ev = $lines | Where-Object { $_ -match '\| (service|mcp|skill|subagent|tool|pre_step|app_exit|bot_fs|reminder|ui|manage_file|desktop|confirm)\.' }
  if ($ev) { $ev | ForEach-Object { Write-Host $_ } } else { Write-Host '(no matching events in this range)' }

  Write-Host ''
  Write-Host '-- free-form skill lines --'
  $sk = $lines | Where-Object { $_ -match '^(skill_|confirm|skill_dsl_)' }
  if ($sk) { $sk | ForEach-Object { Write-Host $_ } } else { Write-Host '(none)' }

  Write-Host ''
  Write-Host '-- tool calls and failures --'
  $tl = $lines | Where-Object { $_ -match 'tool\.(call|return|panic)' }
  if ($tl) { $tl | Select-Object -Last 40 | ForEach-Object { Write-Host $_ } } else { Write-Host '(none)' }
}

# ------------------------------------------------------ managed services

# The reliable pid source is the service.started event in bot.log; stdout.log
# only holds the service's own output, not its pid.
function Get-ServicePids {
  param([string]$Dir)
  $log = Get-BotLogPath $Dir
  if (-not (Test-Path $log)) { return @() }
  $out = @()
  foreach ($line in (Select-String -Path $log -Pattern 'service\.started' -ErrorAction SilentlyContinue)) {
    $sid = ''
    $procId = ''
    if ($line.Line -match 'service_id=([^ |]+)') { $sid = $Matches[1] }
    if ($line.Line -match 'pid=(\d+)') { $procId = $Matches[1] }
    if ($sid -and $procId) { $out += [pscustomobject]@{ Id = $sid; Pid = [int]$procId } }
  }
  return $out
}

function Get-ListenPorts {
  param([int]$ProcId)
  $rows = @()
  try {
    $rows = Get-NetTCPConnection -OwningProcess $ProcId -State Listen -ErrorAction Stop |
      Select-Object -Property LocalAddress, LocalPort
  } catch {
    # Fall back to netstat on systems without the NetTCPIP module.
    $out = & netstat -ano 2>$null | Select-String -Pattern "\s$ProcId\s*$"
    foreach ($o in $out) {
      $line = $o.ToString()
      if ($line -notmatch 'LISTENING') { continue }
      $parts = ($line -split '\s+') | Where-Object { $_ }
      if ($parts.Count -ge 2 -and $parts[0] -eq 'TCP') {
        $rows += [pscustomobject]@{ LocalAddress = $parts[1]; LocalPort = ($parts[1] -split ':')[-1] }
      }
    }
  }
  return $rows
}

function Cmd-Services {
  param([string]$Dir)
  $root = Get-ServicesRoot
  Write-Host "-- service log root: $root --"
  $svcs = Get-ServicePids $Dir
  if (-not $svcs) {
    Write-Host 'no service.started event in bot.log - no managed service was started in this run.'
  }
  foreach ($s in $svcs) {
    Write-Host "-- $($s.Id)  pid=$($s.Pid) --"
    $p = Get-Process -Id $s.Pid -ErrorAction SilentlyContinue
    if ($p) {
      Write-Host "  process         = $($p.ProcessName)"
      Write-Host "  started         = $($p.StartTime)"
      $ports = Get-ListenPorts $s.Pid
      if ($ports) {
        foreach ($pt in $ports) { Write-Host "  listening       = $($pt.LocalAddress):$($pt.LocalPort)" }
      } else {
        Write-Host '  listening       = (no LISTEN port found)'
      }
    } else {
      Write-Host "  process $($s.Pid) is gone (exited / stopped / killed with the host)"
    }
  }

  if ($svcs -and (Test-Path $root)) {
    Write-Host ''
    Write-Host '-- service output tails --'
    Get-ChildItem -Path $root -Directory -ErrorAction SilentlyContinue | ForEach-Object {
      Write-Host "-- $($_.Name) --"
      $out = Join-Path $_.FullName 'stdout.log'
      $err = Join-Path $_.FullName 'stderr.log'
      if (Test-Path $out) { Get-Content $out -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  out| $_" } }
      if (Test-Path $err) { Get-Content $err -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  err| $_" } }
    }
  }
}

function Cmd-Orphans {
  param([string]$Dir)
  Write-Host '-- orphan check --'
  $hosts = Get-Process -Name 'wmessage' -ErrorAction SilentlyContinue
  if ($hosts) {
    Write-Host "host pids        = $(($hosts | ForEach-Object { $_.Id }) -join ' ')"
  } else {
    Write-Host 'host pids        = (no running wmessage found)'
  }
  $found = $false
  foreach ($s in (Get-ServicePids $Dir)) {
    if (Get-Process -Id $s.Pid -ErrorAction SilentlyContinue) {
      $found = $true
      Write-Host "  [!] $($s.Id) pid=$($s.Pid) is STILL ALIVE" -ForegroundColor Yellow
      Get-Process -Id $s.Pid -ErrorAction SilentlyContinue |
        Select-Object Id, ProcessName, StartTime | Format-Table -AutoSize | Out-String | Write-Host
    }
  }
  if (-not $found) { Write-Host '  [ok] no orphan service processes' }
  Write-Host '  (the watchdog may need a few seconds after the host dies - retry in 5-10s)'
}

function Cmd-Keyring {
  Write-Host '-- Windows credential manager entries (target name: mcp:<server id>) --'
  $hit = & cmdkey /list 2>$null | Select-String -Pattern 'mcp:'
  if ($hit) {
    $hit | ForEach-Object { Write-Host "  $($_.ToString().Trim())" }
    Write-Host ''
    Write-Host '  cmdkey /list shows target names only - the secret value cannot be read back.'
  } else {
    Write-Host '  (no mcp: entries found)'
  }
}

function Cmd-Fixes {
  param([string]$Dir)
  $dataDir = Resolve-DataDir $Dir
  $log = Join-Path $dataDir 'bot.log'
  Write-Host '-- fix re-verification evidence --'
  Write-Host "  data_dir = $dataDir"
  if (-not (Test-Path $log)) {
    Write-Host '  [x] bot.log not found - run locate first'
    return
  }

  Write-Host '-- D-2: open_url / fetch_url failure level (same cause must be same level) --'
  $toolLines = Select-String -Path $log -Pattern 'tool\.return .*tool=(open_url|fetch_url)' -Encoding UTF8
  if ($toolLines) {
    $toolLines | Select-Object -Last 8 | ForEach-Object { Write-Host "  $($_.ToString().Trim())" }
    Write-Host '  expect: a DNS failure shows ERROR for BOTH open_url and fetch_url'
  } else {
    Write-Host '  (no open_url / fetch_url tool.return lines yet)'
  }

  Write-Host '-- D-6: ready timeout attribution --'
  $svcLines = Select-String -Path $log -Pattern 'service\.(started|ready|stop_requested|exited|stopped|start_failed)' -Encoding UTF8
  if ($svcLines) {
    $svcLines | Select-Object -Last 15 | ForEach-Object { Write-Host "  $($_.ToString().Trim())" }
    Write-Host '  expect: start_failed reason = "not ready within Ns", never "exited immediately"'
    Write-Host '  expect: the exited line carries stop_reason=ready_timeout'
  } else {
    Write-Host '  (no service lifecycle lines yet)'
  }

  Write-Host '-- D-1: build machine path leak (must be empty) --'
  $leak = Select-String -Path $log -Pattern '/Users/|/home/[a-z]+/' -Encoding UTF8
  if ($leak) {
    $leak | Select-Object -Last 5 | ForEach-Object { Write-Host "  [!] $($_.ToString().Trim())" }
  } else {
    Write-Host '  [ok] no build machine path in bot.log'
  }

  Write-Host '-- human-eye items (UIAutomation cannot read the WebView2 DOM) --'
  Write-Host '  D-7 panel: the services panel must show the capability line and NO "total memory 0 MB"'
  Write-Host '  D-8 screenshot: taking a screenshot must not pop a console window'
}

function Cmd-Usage {
  Write-Host @'
Usage:
  powershell -ExecutionPolicy Bypass -File regression-probe.ps1 <subcommand> [-ExeDir "<package dir>"]

  locate    resolve data_dir (portable dir vs %APPDATA% fallback) / bot.log / service log root
  mark      stamp a byte watermark on bot.log
  report    dump audit events recorded after the watermark
  services  list managed-service processes and their listening ports
  orphans   find services still alive after the host is gone
  keyring   list MCP credential-manager entries
  fixes     dump the log evidence for the fixed defects (D-1 / D-2 / D-6)
  usage     this help

The source is ASCII-only on purpose: PowerShell 5.1 reads .ps1 as ANSI
(GBK on zh-CN Windows) unless a UTF-8 BOM is present, and mangled
multi-byte sequences can break string literals.
'@
}

$resolvedExe = Resolve-ExeDir $ExeDir

switch ($Command.ToLower()) {
  'locate'   { Cmd-Locate $resolvedExe }
  'mark'     { Cmd-Mark $resolvedExe }
  'report'   { Cmd-Report $resolvedExe }
  'services' { Cmd-Services $resolvedExe }
  'orphans'  { Cmd-Orphans $resolvedExe }
  'keyring'  { Cmd-Keyring }
  'fixes'    { Cmd-Fixes $resolvedExe }
  'usage'    { Cmd-Usage }
  default    { Cmd-Usage; exit 1 }
}
