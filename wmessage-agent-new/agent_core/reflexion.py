"""Reflexion 反思循环 — Phase 5 第 1 项。

设计：
- 任务结束后（status=ok 或 failed）触发反思
- 失败任务：生成 what_went_wrong / why / improvement（LLM 生成 或 规则引擎 fallback）
- 成功任务：存简短记录（监盘用）
- 反思结果存 `reflexion_memory` 表（SQLite via ReflexionDAO）
- 下一轮 chat：`recall_relevant(task, top_k=3)` 检索相似任务反思，注入 system prompt

开关：`feature_flags.reflexion_enabled = false` 默认关（避免 LLM token 消耗）
启用后每个 task 完成都触发（含成功 task 的简短记录）
"""
from __future__ import annotations

import json
import time
from dataclasses import asdict, dataclass, field
from typing import Any, Dict, List, Optional

from agent_core.db import ReflexionDAO, connect


@dataclass
class Reflexion:
    """单条反思记录。"""

    task_run_id: Optional[str]
    task_title: str
    task_status: str  # "ok" | "failed" | "in_progress"
    what_went_wrong: str = ""
    why: str = ""  # 错误分类（timeout / verify_failed / llm_error / unknown）
    improvement: str = ""
    ts_ms: int = 0
    extra: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


# ───────────────────────── Reflexion 引擎 ─────────────────────────


class ReflexionEngine:
    """反思循环主类。

    Args:
        db_path: SQLite 数据库路径（reflexion_memory 表）
        llm: 可选 LLM client — 提供则用 LLM 生成反思，否则用规则引擎 fallback
    """

    def __init__(self, db_path: str, llm=None):
        self.db_path = db_path
        self.llm = llm
        self._dao_cache: Optional[ReflexionDAO] = None

    def _dao(self) -> ReflexionDAO:
        if self._dao_cache is None:
            conn = connect(self.db_path)
            self._dao_cache = ReflexionDAO(conn)
        return self._dao_cache

    def close(self):
        if self._dao_cache is not None:
            try:
                self._dao_cache.conn.close()
            except Exception:
                pass
            self._dao_cache = None

    # ───────────────────────── 反思生成 ─────────────────────────

    def reflect(
        self,
        task,
        task_result_status: str,
        task_result_error: Optional[str] = None,
        task_run_id: Optional[str] = None,
    ) -> Reflexion:
        """对完成的任务生成反思。

        Args:
            task: TaskNode（已完成）
            task_result_status: "ok" | "failed" | "in_progress"
            task_result_error: 错误详情（仅 failed 时有）
            task_run_id: 关联的 skill_run_id

        Returns:
            Reflexion 对象（已存 DB）
        """
        ts_ms = int(time.time() * 1000)

        if task_result_status == "ok":
            # 成功任务：简短记录
            return self._store(Reflexion(
                task_run_id=task_run_id,
                task_title=getattr(task, "title", "unknown"),
                task_status="ok",
                what_went_wrong="",
                why="",
                improvement="",
                ts_ms=ts_ms,
            ))

        # 失败任务：生成完整反思
        what_wrong = task_result_error or "unknown"
        if self.llm is not None:
            # LLM 生成反思（Phase 5 后续优化：更精准的反思）
            try:
                why, improvement = self._llm_reflect(task, what_wrong)
            except Exception:
                # LLM 失败 → fallback 规则引擎
                why, improvement = self._rule_reflect(what_wrong)
        else:
            why, improvement = self._rule_reflect(what_wrong)

        return self._store(Reflexion(
            task_run_id=task_run_id,
            task_title=getattr(task, "title", "unknown"),
            task_status="failed",
            what_went_wrong=what_wrong,
            why=why,
            improvement=improvement,
            ts_ms=ts_ms,
        ))

    def _rule_reflect(self, error: str) -> tuple:
        """规则引擎 fallback（无 LLM 时）：基于错误关键词生成 why + improvement。"""
        err_lower = error.lower()
        if "timeout" in err_lower or "超时" in error:
            return (
                "timeout",
                "考虑增加 timeout / 减少 max_steps / 用更小模型",
            )
        if "verify" in err_lower or "断言" in error or "assert" in err_lower:
            return (
                "verify_failed",
                "检查 verify_hint 是否合理 / 调整 verify_template / 重写 task 让 verify 更宽松",
            )
        if "module" in err_lower or "import" in err_lower:
            return (
                "missing_dependency",
                "检查依赖是否安装完整 / 添加缺失的 import",
            )
        if "permission" in err_lower or "权限" in error:
            return (
                "permission_denied",
                "检查文件权限 / 调整 tool_guard 白名单",
            )
        if "command" in err_lower or "exit" in err_lower:
            return (
                "command_failed",
                "检查 verify_hint 的 command 语法 / 退出码预期是否合理",
            )
        return ("unknown", "需要人工分析错误日志")

    def _llm_reflect(self, task, error: str) -> tuple:
        """LLM 生成反思（更精准但慢）。"""
        from llm_provider.base import LLMRequest

        prompt = (
            "你是 wmessage-agent 反思引擎。任务失败，分析原因并给改进建议。\n\n"
            f"任务: {task.title}\n"
            f"错误: {error[:500]}\n\n"
            "返回 JSON: {\"why\": \"<简短分类>\", \"improvement\": \"<具体改进>\"}\n"
        )
        resp = self.llm.complete(LLMRequest(user=prompt, temperature=0.3, max_tokens=200))
        try:
            data = json.loads(resp.text)
            return data.get("why", "unknown"), data.get("improvement", "")
        except (json.JSONDecodeError, KeyError):
            return self._rule_reflect(error)

    # ───────────────────────── 存储 ─────────────────────────

    def _store(self, refl: Reflexion) -> Reflexion:
        """存反思到 SQLite。"""
        self._dao().insert(
            task_run_id=refl.task_run_id,
            task_title=refl.task_title,
            task_status=refl.task_status,
            what_went_wrong=refl.what_went_wrong,
            why=refl.why,
            improvement=refl.improvement,
            ts_ms=refl.ts_ms or int(time.time() * 1000),
            extra=refl.extra,
        )
        return refl

    # ───────────────────────── 检索（注入 system prompt）─────────────────────────

    def recall_relevant(
        self,
        current_task_title: str,
        top_k: int = 3,
    ) -> List[Reflexion]:
        """检索与当前任务相关的历史反思。

        简单实现：按时间倒序 + 标题子串匹配（后续 Phase 5.5 可升级为 embedding 相似度）。
        """
        all_refs = self._dao().list_recent(limit=top_k * 5)
        # 优先：标题包含当前任务关键词的
        keywords = set(current_task_title.lower().split())
        scored = []
        for r in all_refs:
            score = sum(1 for kw in keywords if kw in r["task_title"].lower())
            scored.append((score, r))
        scored.sort(key=lambda x: (-x[0], -x[1]["ts_ms"]))
        return [r for _, r in scored[:top_k]]

    def format_for_prompt(self, reflections: List[Dict[str, Any]]) -> str:
        """格式化反思列表为 system prompt 注入文本。"""
        if not reflections:
            return ""
        lines = ["## 历史反思（相关任务经验）"]
        for r in reflections:
            status = r["task_status"]
            icon = "✅" if status == "ok" else "❌"
            lines.append(f"\n### {icon} {r['task_title']}（{status}）")
            if r.get("what_went_wrong"):
                lines.append(f"- 错误: {r['what_went_wrong'][:200]}")
            if r.get("why"):
                lines.append(f"- 原因: {r['why']}")
            if r.get("improvement"):
                lines.append(f"- 改进: {r['improvement']}")
        return "\n".join(lines)
