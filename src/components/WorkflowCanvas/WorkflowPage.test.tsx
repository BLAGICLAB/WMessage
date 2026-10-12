import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkflowPage } from "./WorkflowPage";
import type { WorkflowSaveResult } from "../../types";

// jsdom 无 ResizeObserver：React Flow 画布尺寸观测需要（仅桩住可实例化）
if (typeof globalThis.ResizeObserver === "undefined") {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver =
    ResizeObserverStub as unknown as typeof ResizeObserver;
}

// vi.mock 工厂会被提升到顶部，共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  return { invokeMock };
});

// 默认实现（具名：beforeEach 只 mockClear 不重置 implementation）
const defaultInvoke = async (cmd: string, _args?: Record<string, unknown>) => {
  switch (cmd) {
    case "workflow_list":
      return [{ id: "wf1", name: "已有工作流", goal: "总目标" }];
    case "workflow_load":
      return {
        id: "wf1",
        name: "已有工作流",
        goal: "总目标",
        tasks: [],
        attachments: null,
        // askMode "key" = 提问开关开；打开后 state 与快照必须同源 → 不脏
        clarifyMeta: JSON.stringify({
          answers: [{ question: "缺哪张表？", answer: "users 表" }],
          askMode: "key",
        }),
      };
    case "workflow_save": {
      const saved: WorkflowSaveResult = {
        workflowId: "wf1",
        bindings: [],
        kept: 0,
        created: 0,
        deleted: 0,
      };
      return saved;
    }
    case "bot_get_config":
      return { modelsByProvider: null };
    default:
      return null;
  }
};
mocks.invokeMock.mockImplementation(defaultInvoke);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
}));

const noop = () => {};

const renderPage = () =>
  render(
    <WorkflowPage
      tasks={[]}
      onSetColumn={noop}
      onUpdate={noop}
      onTasksReload={noop}
    />,
  );

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.invokeMock.mockImplementation(defaultInvoke);
});

describe("WorkflowPage dirty 快照", () => {
  it("打开已保存工作流不脏；仅切执行提问开关即脏且能保存，开关随 clarify_meta 落库", async () => {
    const user = userEvent.setup();
    renderPage();
    // 工作流列表已进下拉
    const select = screen.getByLabelText("切换工作流");
    await waitFor(() => {
      expect(
        within(select).getByRole("option", { name: "已有工作流" }),
      ).toBeInTheDocument();
    });
    await user.selectOptions(select, "wf1");
    // 打开完成：提问开关=开（来自 clarify_meta.askMode=key）
    const asksBtn = await screen.findByTitle(/执行提问：开/);
    // 保存前不脏：保存键禁用
    expect(screen.getByTestId("save-btn")).toBeDisabled();
    // 仅切开关 → 变脏：保存键解禁
    await user.click(asksBtn);
    expect(screen.getByTitle(/执行提问：关/)).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId("save-btn")).toBeEnabled();
    });
    // 保存可执行：切换后的 askMode=never 与既有回答一起随 clarify_meta 落库
    const saveBtn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(saveBtn, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(saveBtn, { pointerId: 1 });
    await waitFor(() => {
      const call = mocks.invokeMock.mock.calls.find(
        (c) => c[0] === "workflow_save",
      );
      expect(call).toBeDefined();
      const input = (call as [string, { input: { clarifyMeta: string } }])[1]
        .input;
      const meta = JSON.parse(input.clarifyMeta) as {
        askMode: string;
        answers: Array<{ question: string; answer: string }>;
      };
      expect(meta.askMode).toBe("never");
      expect(meta.answers).toEqual([
        { question: "缺哪张表？", answer: "users 表" },
      ]);
    });
  });
});
