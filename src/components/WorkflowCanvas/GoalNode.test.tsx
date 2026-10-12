import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import { GoalNode, type GoalNodeData } from "./GoalNode";

const baseData: GoalNodeData = {
  name: "测试工作流",
  goal: "把事情做成",
  saved: true,
  progress: null,
  report: null,
  assumptions: null,
  onRename: vi.fn(),
  onGoalChange: vi.fn(),
};

describe("GoalNode", () => {
  it("拆解假设含重复文本：条数正确且无重复 key 告警（LLM 自由文本不作 key）", () => {
    const errSpy = vi.spyOn(console, "error");
    // Handle（@xyflow/react）要求 Provider 祖先，与 WorkflowPage 挂载方式一致
    render(
      <ReactFlowProvider>
        <GoalNode
          data={{
            ...baseData,
            assumptions: ["同一句假设", "同一句假设", "不同的一句"],
          }}
        />
      </ReactFlowProvider>,
    );
    // details 折叠条内容始终在 DOM：三条假设一条不少
    expect(screen.getAllByText("同一句假设")).toHaveLength(2);
    expect(screen.getByText("不同的一句")).toBeInTheDocument();
    // 重复文本作 key 会触发 React「same key」告警——索引 key 下不得出现
    expect(
      errSpy.mock.calls.filter((args) =>
        args.some((a) => typeof a === "string" && a.includes("same key")),
      ),
    ).toHaveLength(0);
  });
});
