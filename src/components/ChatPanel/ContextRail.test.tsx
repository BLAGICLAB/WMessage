import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ContextRail, type SubagentRun } from "./ContextRail";
import type { Task } from "../../types";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

const task: Task = { id: "t1", title: "写周报", column: "todo" };

function renderRail(overrides: Partial<Parameters<typeof ContextRail>[0]> = {}) {
  return render(
    <ContextRail
      tasks={[task]}
      onUpdateTask={vi.fn()}
      onRefTask={vi.fn()}
      refIds={[]}
      fileChanges={[]}
      subagents={[]}
      onOpenSubagent={vi.fn()}
      onCancelSubagent={vi.fn()}
      tab="tasks"
      onTabChange={vi.fn()}
      {...overrides}
    />,
  );
}

beforeEach(() => {
  mocks.invokeMock.mockReset();
});

// 聊天右侧栏绑卡入口：任务池行内回形针 → pick_files_dialog → bind_files →
// 统一内核合并（去重/文件夹槽替换/10 上限）→ onUpdateTask 既有落库通道
describe("ContextRail 绑定文件到任务卡", () => {
  it("点回形针：选文件后绑卡，合并结果经 onUpdateTask 落库", async () => {
    const user = userEvent.setup();
    const onUpdateTask = vi.fn();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "pick_files_dialog")
        return ["/Users/t/Desktop/周报.docx"];
      if (cmd === "bind_files")
        return [{ path: "/Users/t/Desktop/周报.docx", isDir: false }];
      return null;
    });
    renderRail({ onUpdateTask });
    await user.click(screen.getByLabelText("绑定文件到 写周报"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("pick_files_dialog");
      expect(mocks.invokeMock).toHaveBeenCalledWith("bind_files", {
        paths: ["/Users/t/Desktop/周报.docx"],
      });
    });
    expect(onUpdateTask).toHaveBeenCalledWith("t1", {
      files: [{ path: "/Users/t/Desktop/周报.docx", isDir: false }],
    });
  });

  it("对话框取消（空列表）不触发 bind_files 也不写库", async () => {
    const user = userEvent.setup();
    const onUpdateTask = vi.fn();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "pick_files_dialog") return [];
      return null;
    });
    renderRail({ onUpdateTask });
    await user.click(screen.getByLabelText("绑定文件到 写周报"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("pick_files_dialog");
    });
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bind_files", {
      paths: expect.anything(),
    });
    expect(onUpdateTask).not.toHaveBeenCalled();
  });

  it("绑定的路径已在卡上：merge 无新增，不写库", async () => {
    const user = userEvent.setup();
    const onUpdateTask = vi.fn();
    const bound: Task = {
      ...task,
      files: [{ path: "/Users/t/Desktop/周报.docx", isDir: false }],
    };
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "pick_files_dialog")
        return ["/Users/t/Desktop/周报.docx"];
      if (cmd === "bind_files")
        return [{ path: "/Users/t/Desktop/周报.docx", isDir: false }];
      return null;
    });
    renderRail({ tasks: [bound], onUpdateTask });
    await user.click(screen.getByLabelText("绑定文件到 写周报"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bind_files", {
        paths: ["/Users/t/Desktop/周报.docx"],
      });
    });
    expect(onUpdateTask).not.toHaveBeenCalled();
  });
});

// 六态标签曾在本文件与 ChatPanel 的收尾提示各写一份，同一状态两处措辞不同
// （succeeded 一处「完成」一处「已完成」）。现在共用 constants 一份，这里钉住
// 侧栏渲染出来的就是那一份
describe("ContextRail 子 Agent 状态标签", () => {
  it("六态徽标文案 = constants 共享表", () => {
    const STATES: [SubagentRun["status"], string][] = [
      ["queued", "排队中"],
      ["running", "执行中"],
      ["succeeded", "完成"],
      ["failed", "失败"],
      ["cancelled", "已取消"],
      ["budget_exceeded", "预算耗尽"],
    ];
    renderRail({
      tab: "agents",
      subagents: STATES.map(([status], i) => ({
        id: `sa_${i}`,
        title: `子任务${i}`,
        status,
        running: status === "running" || status === "queued",
      })),
    });
    for (const [, label] of STATES) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });
});
