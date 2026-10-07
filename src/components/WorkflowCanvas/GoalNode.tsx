import { memo } from "react";
import { Handle, Position } from "@xyflow/react";
import { Play } from "lucide-react";
import type { WorkflowReport } from "../../types";

/** 总目标卡（W1-CANVAS 设计 §决策4/§5）：绑定 workflows 行元数据（非 Task），
 *  兼作运行入口——▶ 按钮 W1 置灰（执行引擎 W3 上线）。固定尺寸，不参与连线。
 *  索引签名：React Flow v12 要求 node data 满足 Record<string, unknown> */
export interface GoalNodeData extends Record<string, unknown> {
  name: string;
  goal: string;
  /** null = 未保存过（新工作流） */
  saved: boolean;
  /** 执行进度（W3）：done/total；null = 不显示 */
  progress: { done: number; total: number } | null;
  /** W-QA：上轮执行的收尾审校报告（rubric 评审；workflow-report 事件 / lastReport 列） */
  report: WorkflowReport | null;
  /** W9-ASK：本次拆解的模型假设（decompose 返回；null/空 = 不显示） */
  assumptions: string[] | null;
  onRename: (name: string) => void;
  onGoalChange: (goal: string) => void;
}

/** verdict → 徽标文案/色调（unknown = 评审降级为纯文本，不判色） */
function verdictBadge(verdict: string): { label: string; cls: string } {
  switch (verdict) {
    case "pass":
      return { label: "✅ 达标", cls: "text-[var(--ok,#22c55e)]" };
    case "partial":
      return { label: "🟡 部分达标", cls: "text-[var(--warn,#eab308)]" };
    case "fail":
      return { label: "❌ 未达标", cls: "text-[var(--danger,#ef4444)]" };
    default:
      return { label: "📋 报告", cls: "text-[var(--t5)]" };
  }
}

export const GoalNode = memo(function GoalNode({ data }: { data: GoalNodeData }) {
  const badge = data.report ? verdictBadge(data.report.verdict) : null;
  return (
    <div className="nm-inset w-[380px] rounded-2xl p-4">
      <Handle type="target" position={Position.Top} className="!opacity-0" />
      <div className="flex items-center gap-2">
        <input
          aria-label="工作流名称"
          className="min-w-0 flex-1 rounded-[var(--r-sm)] bg-transparent text-sm font-semibold text-[var(--t1)] outline-none"
          value={data.name}
          placeholder="未命名工作流"
          onChange={(e) => data.onRename(e.target.value)}
        />
        <button
          className="flex shrink-0 items-center gap-1 rounded-full px-3 py-1 text-xs text-[var(--t5)] nm-outset"
          title="执行引擎将在 W3 批次上线"
          disabled
        >
          <Play size={12} aria-hidden /> 运行
        </button>
      </div>
      <textarea
        aria-label="工作流目标"
        className="mt-2 h-16 w-full resize-none rounded-[var(--r-sm)] bg-transparent text-xs leading-5 text-[var(--t3)] outline-none placeholder:text-[var(--t5)]"
        value={data.goal}
        placeholder="这个工作流要达成什么目标？"
        onChange={(e) => data.onGoalChange(e.target.value)}
      />
      {/* 只有进度/未保存才有这一行——report 独占时不渲染空行（会顶出一段空白） */}
      {(data.progress || !data.saved) && (
        <div className="mt-1 flex items-center justify-between">
          {data.progress && (
            <span className="text-[10px] text-[var(--t5)]">
              ✔ {data.progress.done}/{data.progress.total} 节点完成
            </span>
          )}
          {!data.saved && (
            <span className="text-[10px] text-[var(--t5)]">未保存——点工具栏「保存」落库</span>
          )}
        </div>
      )}
      {/* W9-ASK：拆解假设（折叠展示——模型拆解时"想当然"的部分摆上台面，
          用户看到错误假设就知道该改哪张卡或用澄清回答重新生成） */}
      {data.assumptions && data.assumptions.length > 0 && (
        <details className="nm-card mt-2 rounded-xl px-2 py-1.5 text-xs">
          <summary className="cursor-pointer select-none text-[var(--t4)]">
            🤖 拆解假设（{data.assumptions.length}）
          </summary>
          <ul className="mt-1 list-inside list-disc text-[var(--t4)]">
            {data.assumptions.map((a) => (
              <li key={a} className="break-words">
                {a}
              </li>
            ))}
          </ul>
        </details>
      )}
      {/* W-QA：收尾审校报告（折叠展示；返工终审会覆盖更新） */}
      {data.report && badge && (
        <details className="nm-card mt-2 rounded-xl px-2 py-1.5 text-xs">
          <summary className="cursor-pointer select-none text-[var(--t4)]">
            <span className={badge.cls}>{badge.label}</span>
            {data.report.reworkRound && (
              <span className="ml-1 text-[var(--t5)]">（审校返工后终审）</span>
            )}
          </summary>
          {data.report.overall && (
            <p className="mt-1 whitespace-pre-wrap break-words text-[var(--t4)]">
              {data.report.overall}
            </p>
          )}
          {(data.report.issues ?? []).length > 0 && (
            <ul className="mt-1 list-inside list-disc text-[var(--t4)]">
              {data.report.issues.map((it, i) => (
                <li key={`${it.nodeTitle}-${i}`} className="mt-0.5">
                  「{it.nodeTitle}」{it.problem}
                  {it.needsRework && <span className="text-[var(--warn,#eab308)]">（已触发返工）</span>}
                </li>
              ))}
            </ul>
          )}
        </details>
      )}
    </div>
  );
});
