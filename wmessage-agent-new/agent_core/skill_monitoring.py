"""skill_monitoring — 查询 audit_event 表获取最近 skill outcomes。

Phase 4 step 2：给 Tauri 设置页 SkillsPanel 提供数据源。

设计：
- 纯函数 `query_recent_skill_outcomes(db_path, skill_name, limit)` 从 SQLite audit_event 表查
- 不依赖 LLM / IPC，只读 DB
- 返回结构化 list[dict]，CLI / IPC / Tauri 都能用

关联事件：
- `skill.start`  → 每个 skill_run 的开始
- `skill.finish` → 每个 skill_run 的结束，payload 里有 status / end_reason

输出 outcome 字段：
- skill_run_id, skill_name, mode, status, start_ts, end_ts, duration_ms
"""
from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Dict, List, Optional

from agent_core.db import connect


def query_recent_skill_outcomes(
    db_path: str,
    skill_name: Optional[str] = None,
    limit: int = 10,
) -> List[Dict[str, Any]]:
    """查询最近的 skill outcomes（start + finish 配对）。

    Args:
        db_path: agent.db SQLite 路径
        skill_name: 只返回指定 skill 的 outcome（None = 全部）
        limit: 返回数量上限

    Returns:
        list of dict，每个 dict：
            {
                "skill_run_id": "sr-uuid",
                "skill_name": "minimax-docx",
                "mode": "auto",
                "status": "ok" | "failed" | "in_progress",
                "end_reason": "ok" | "failed" | "verify_failed" | "replan_exhausted" | ...,
                "started_at": "2026-08-22T20:59:44",  # ISO 8601
                "finished_at": "2026-08-22T20:59:45",
                "duration_ms": 1000,
            }
        按 start_ts 降序排，最新的在前。
    """
    conn = connect(db_path)
    try:
        cur = conn.execute(
            """
            SELECT event, skill_run_id, payload, ts_ms
            FROM audit_event
            WHERE event IN ('skill.start', 'skill.finish')
              AND skill_run_id IS NOT NULL
            ORDER BY ts_ms DESC
            LIMIT ?
            """,
            (max(limit * 4, 40),),
        )
        rows = cur.fetchall()
    finally:
        conn.close()

    # 按 skill_run_id 分组
    starts: Dict[str, Dict[str, Any]] = {}  # sr_id → {payload, ts_ms}
    finishes: Dict[str, Dict[str, Any]] = {}
    for event, sr_id, payload_json, ts_ms in rows:
        if not sr_id:
            continue
        try:
            payload = json.loads(payload_json) if payload_json else {}
        except (json.JSONDecodeError, TypeError):
            payload = {}
        target = starts if event == "skill.start" else finishes
        target[sr_id] = {"payload": payload, "ts_ms": ts_ms}

    # Build outcomes（按 start_ts 降序）
    outcomes: List[Dict[str, Any]] = []
    for sr_id in starts:
        start_info = starts[sr_id]
        skill = start_info["payload"].get("skill", "unknown")
        if skill_name is not None and skill != skill_name:
            continue

        base = {
            "skill_run_id": sr_id,
            "skill_name": skill,
            "mode": start_info["payload"].get("mode", ""),
            "started_at": _fmt_ts(start_info["ts_ms"]),
            "started_ts_ms": start_info["ts_ms"],
        }

        if sr_id in finishes:
            finish_info = finishes[sr_id]
            fp = finish_info["payload"]
            duration_ms = finish_info["ts_ms"] - start_info["ts_ms"]
            outcomes.append({
                **base,
                "status": fp.get("status", "unknown"),
                "end_reason": fp.get("end_reason", ""),
                "finished_at": _fmt_ts(finish_info["ts_ms"]),
                "finished_ts_ms": finish_info["ts_ms"],
                "duration_ms": max(0, duration_ms),
            })
        else:
            # in_progress: started but no finish event yet
            outcomes.append({
                **base,
                "status": "in_progress",
                "end_reason": "",
                "finished_at": None,
                "finished_ts_ms": None,
                "duration_ms": None,
            })

    # Already ordered by start ts DESC (because rows were ordered)
    # Apply limit
    return outcomes[:limit]


def _fmt_ts(ts_ms: int) -> str:
    """ms epoch → ISO 8601 字符串。"""
    return datetime.fromtimestamp(ts_ms / 1000).isoformat()


def format_outcomes_table(outcomes: List[Dict[str, Any]]) -> str:
    """格式化 outcomes 为可读表格（CLI 输出用）。"""
    if not outcomes:
        return "(no skill outcomes found)"

    headers = ["skill_run_id", "skill_name", "mode", "status", "duration_ms", "started"]
    rows = []
    for o in outcomes:
        dur = f"{o['duration_ms']}ms" if o.get("duration_ms") is not None else "-"
        started = o.get("started_at", "-")[:19]  # 截到秒
        sr = o["skill_run_id"][:18]
        rows.append([
            sr,
            o["skill_name"],
            o.get("mode", ""),
            o["status"],
            dur,
            started,
        ])

    widths = [
        max(len(str(r[i])) for r in [headers] + rows)
        for i in range(len(headers))
    ]
    lines = []
    lines.append(" | ".join(h.ljust(w) for h, w in zip(headers, widths)))
    lines.append("-+-".join("-" * w for w in widths))
    for r in rows:
        lines.append(" | ".join(str(c).ljust(w) for c, w in zip(r, widths)))
    return "\n".join(lines)


def summarize_status(outcomes: List[Dict[str, Any]]) -> Dict[str, int]:
    """汇总 status 分布 — 给状态徽章用。"""
    summary: Dict[str, int] = {}
    for o in outcomes:
        s = o.get("status", "unknown")
        summary[s] = summary.get(s, 0) + 1
    return summary
