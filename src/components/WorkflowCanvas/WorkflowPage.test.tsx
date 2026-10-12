import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
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

describe("WorkflowPage startRun 竞态", () => {
  it("切换后晚到的 run 失败：不压新画布 running 态、不误弹错误（只留痕）", async () => {
    const user = userEvent.setup();
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    // 受控 promise：workflow_run 挂起，测试内手动 reject 模拟晚到失败
    let rejectRun!: (e: unknown) => void;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        switch (cmd) {
          case "workflow_list":
            return [
              { id: "wf1", name: "工作流一", goal: "" },
              { id: "wf2", name: "工作流二", goal: "" },
            ];
          case "workflow_load":
            return {
              id: args?.id,
              name: args?.id === "wf2" ? "工作流二" : "工作流一",
              goal: "",
              // wf1 带节点让执行键可用（空画布执行键禁用）；wf2 只需要执行态
              tasks:
                args?.id === "wf2"
                  ? []
                  : [{ id: "n1", title: "节点一", dependsOn: [] }],
              attachments: null,
              clarifyMeta: null,
            };
          case "workflow_run":
            return new Promise((_resolve, reject) => {
              rejectRun = reject;
            });
          case "workflow_is_running":
            // 新画布 wf2 处于执行态：running=true 是晚到失败不得压掉的状态
            return args?.workflowId === "wf2";
          case "bot_get_config":
            return { modelsByProvider: null };
          default:
            return null;
        }
      },
    );
    renderPage();
    const select = screen.getByLabelText("切换工作流");
    await waitFor(() => {
      expect(
        within(select).getByRole("option", { name: "工作流二" }),
      ).toBeInTheDocument();
    });
    await user.selectOptions(select, "wf1");
    // wf1 打开且不脏：执行键就绪
    await waitFor(() => {
      expect(screen.getByTestId("exec-bar")).toHaveAttribute(
        "data-phase",
        "ready",
      );
    });
    await user.click(screen.getByTestId("exec-bar"));
    expect(typeof rejectRun).toBe("function"); // workflow_run 已挂起
    // 切到 wf2：is_running=true → 新画布进入执行态
    await user.selectOptions(select, "wf2");
    await waitFor(() => {
      expect(screen.getByTestId("exec-bar")).toHaveAttribute(
        "data-phase",
        "running",
      );
    });
    // wf1 的 run 晚到失败：不得置新画布 running=false，不得误弹错误归属
    rejectRun(new Error("stale run failed"));
    await act(async () => {}); // flush：让 catch 分支跑完
    expect(screen.getByTestId("exec-bar")).toHaveAttribute(
      "data-phase",
      "running",
    );
    expect(alertSpy).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // 跳过的失败仍留痕（console），不静默吞掉
    expect(errSpy).toHaveBeenCalledWith(
      expect.stringContaining("旧画布开始执行失败"),
      expect.anything(),
    );
  });
});
