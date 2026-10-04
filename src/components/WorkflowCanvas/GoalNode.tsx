import { memo } from "react";
import { Handle, Position } from "@xyflow/react";
import { Play } from "lucide-react";

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
  onRename: (name: string) => void;
  onGoalChange: (goal: string) => void;
}

export const GoalNode = memo(function GoalNode({ data }: { data: GoalNodeData }) {
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
    </div>
  );
});
