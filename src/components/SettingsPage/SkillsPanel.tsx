// SettingsPage 子模块：技能列表 + 状态徽章。
// SkillsPanel：数据目录 skills/<name>/SKILL.md 的导入/删除/打开入口。
// SkillOutcomeBadge：技能最近一次执行结果的颜色 + 图标。

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import type { SkillInfo, SkillOutcome, SkillOutcomeKind } from "./types";

/** Skill 状态徽章颜色 + 图标 */
function SkillOutcomeBadge({ outcome }: { outcome: SkillOutcome }) {
  const map: Record<SkillOutcomeKind, { color: string; label: string; icon: string }> = {
    done: { color: "text-emerald-600 bg-emerald-50", label: "完成", icon: "OK" },
    await_user: { color: "text-blue-600 bg-blue-50", label: "等待确认", icon: "PAUSE" },
    failed_recoverable: { color: "text-amber-600 bg-amber-50", label: "可恢复失败", icon: "WARN" },
    terminated: { color: "text-red-600 bg-red-50", label: "已终止", icon: "STOP" },
  };
  const m = map[outcome.kind];
  const tip = [m.label, outcome.reason, outcome.completedSummary].filter(Boolean).join(" | ");
  return (
    <span
      className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium ${m.color}`}
      title={tip}
    >
      [{m.icon}] {m.label}
    </span>
  );
}

/** 机器人技能管理：导入/删除/打开目录（技能 = 数据目录 skills/<name>/SKILL.md） */
export function SkillsPanel() {
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  /** notice 自动消退计时器：重设前 clear 旧的（防新提示被旧计时提前清掉），卸载清理 */
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
    },
    []
  );

  const refresh = async () => {
    try {
      setSkills(await invoke<SkillInfo[]>("skills_list"));
    } catch (e) {
      // 自动加载失败：只 console 记 code，inline UI 仍展示 message
      handleCommandError(e, "skills_list", { silent: true });
      setError(formatCommandError(e));
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const importSkill = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const selected = await open({ multiple: false, directory: true });
      if (typeof selected !== "string") return;
      const name = await invoke<string>("skills_import", { path: selected });
      setNotice(`技能「${name}」已安装`);
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
      noticeTimer.current = setTimeout(() => setNotice(""), 3000);
      await refresh();
    } catch (e) {
      handleCommandError(e, "skills_import", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const removeSkill = async (name: string) => {
    if (busy) return;
    if (!window.confirm(`删除技能「${name}」？`)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("skills_delete", { name });
      await refresh();
    } catch (e) {
      handleCommandError(e, "skills_delete", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const openDir = async () => {
    try {
      const dir = await invoke<string>("skills_open_dir");
      openPath(dir).catch(() => {});
    } catch (e) {
      handleCommandError(e, "skills_open_dir", { silent: true });
      setError(formatCommandError(e));
    }
  };

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">机器人技能</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        技能 = 一个文件夹（SKILL.md + 可选脚本）。机器人对话时自动看到技能清单，需要时读取完整文档执行
      </p>
      <div className="mt-3 flex items-center gap-2">
        <button className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]" onClick={importSkill} disabled={busy}>
          {busy ? "导入中…" : "⬆ 导入技能文件夹"}
        </button>
        <button className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]" onClick={openDir}>
          📂 打开技能目录
        </button>
        {notice && <span className="text-xs text-[var(--success)]">{notice}</span>}
        {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
      </div>
      {skills.length === 0 ? (
        <p className="mt-3 text-xs text-[var(--t5)]">
          暂无技能：点「导入技能文件夹」选择含 SKILL.md 的文件夹
        </p>
      ) : (
        <div className="mt-3 flex flex-col gap-1.5">
          {skills.map((s) => (
            <div key={s.name} className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]">
                <span className="font-medium text-[var(--t2)]">{s.name}</span>
                {s.description && (
                  <span className="text-[var(--t5)]"> — {s.description}</span>
                )}
              </span>
              {s.lastOutcome && <SkillOutcomeBadge outcome={s.lastOutcome} />}
              <button
                className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                onClick={() => removeSkill(s.name)}
                disabled={busy}
                title="删除技能"
              >
                🗑
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
