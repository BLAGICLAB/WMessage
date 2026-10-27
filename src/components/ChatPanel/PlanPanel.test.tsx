import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PlanPanel } from "./PlanPanel";
import * as planStore from "./planStore";
import type { SubagentRun } from "./ContextRail";

const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  // 面板还订阅 bot-file-changed（环境信息段跟着落盘事件长）
  const listenMock = vi.fn(async () => () => {});
  return { invokeMock, listenMock };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listenMock }));

const run: SubagentRun = {
  id: "sa_1",
  title: "查资料",
  status: "running",
  running: true,
};

function renderPanel(
  overrides: Partial<Parameters<typeof PlanPanel>[0]> = {},
) {
  return render(
    <PlanPanel
      sessionId="s1"
      subagents={[]}
      onClose={vi.fn()}
      {...overrides}
    />,
  );
}

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.listenMock.mockClear();
  // 计划状态在模块级 store 里（跨用例存活），不复位会串味
  planStore.resetAll();
  mocks.invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "bot_plan_state") return null;
    if (cmd === "bot_session_file_changes") return { items: [] };
    return null;
  });
});

describe("计划面板：计划段", () => {
  it("打开时拉当前快照并按三态渲染步骤", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_plan_state")
        return {
          steps: [
            { content: "读文件", status: "completed" },
            { content: "改代码", status: "in_progress" },
            { content: "跑测试", status: "pending" },
          ],
          updatedAt: 100,
          turnStartedAt: 50,
        };
      if (cmd === "bot_session_file_changes") return { items: [] };
      return null;
    });
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText("读文件")).toBeInTheDocument();
    });
    expect(screen.getByText("改代码")).toBeInTheDocument();
    expect(screen.getByText("跑测试")).toBeInTheDocument();
    // 已完成的步骤走删除线，未完成的保持可读
    expect(screen.getByText("读文件").className).toContain("line-through");
    expect(screen.getByText("跑测试").className).not.toContain("line-through");
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_plan_state", {
      sessionId: "s1",
    });
  });

  // 计划事件由共享处理器落进模块级 store（实时与回放补齐共用同一份），
  // 面板读 store——这里直接喂 store，等价于事件到达后的状态。
  it("bot-plan 事件整表替换步骤", async () => {
    renderPanel();
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalled();
    });
    act(() => {
      planStore.applyEvent({
        sessionId: "s1",
        steps: [{ content: "换过的步骤", status: "pending" }],
        updatedAt: 200,
        turnStartedAt: 50,
      });
    });
    expect(screen.getByText("换过的步骤")).toBeInTheDocument();
  });

  it("别的会话的事件不覆盖当前会话的计划", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_plan_state")
        return {
          steps: [{ content: "本会话的步骤", status: "pending" }],
          updatedAt: 100,
          turnStartedAt: 50,
        };
      if (cmd === "bot_session_file_changes") return { items: [] };
      return null;
    });
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText("本会话的步骤")).toBeInTheDocument();
    });
    act(() => {
      planStore.applyEvent({
        sessionId: "s2",
        steps: [{ content: "别的会话的步骤", status: "pending" }],
        updatedAt: 300,
        turnStartedAt: 50,
      });
    });
    expect(screen.queryByText("别的会话的步骤")).not.toBeInTheDocument();
    expect(screen.getByText("本会话的步骤")).toBeInTheDocument();
  });

  it("命令回包比事件旧时不覆盖新状态", async () => {
    // 拉取挂起：先落事件（新的），再放行命令回包（旧的）
    let release: (v: unknown) => void = () => {};
    const pending = new Promise((r) => {
      release = r;
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_plan_state") {
        await pending;
        return {
          steps: [{ content: "旧快照", status: "pending" }],
          updatedAt: 100,
          turnStartedAt: 50,
        };
      }
      if (cmd === "bot_session_file_changes") return { items: [] };
      return null;
    });
    renderPanel();
    act(() => {
      planStore.applyEvent({
        sessionId: "s1",
        steps: [{ content: "新状态", status: "pending" }],
        updatedAt: 300,
        turnStartedAt: 50,
      });
    });
    expect(screen.getByText("新状态")).toBeInTheDocument();
    release(null);
    await act(async () => {
      await pending;
    });
    expect(screen.queryByText("旧快照")).not.toBeInTheDocument();
    expect(screen.getByText("新状态")).toBeInTheDocument();
  });

  it("本轮没更新过时标出状态可能已过期", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_plan_state")
        return {
          steps: [{ content: "上一步的计划", status: "in_progress" }],
          updatedAt: 100,
          turnStartedAt: 500,
        };
      if (cmd === "bot_session_file_changes") return { items: [] };
      return null;
    });
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText(/状态可能已过期/)).toBeInTheDocument();
    });
  });

  it("没有计划时如实说没有，不编一份空清单", async () => {
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText(/本会话暂无计划/)).toBeInTheDocument();
    });
  });
});

describe("计划面板：其余两段", () => {
  it("后台任务段列出子 agent 与状态", async () => {
    renderPanel({ subagents: [run] });
    await waitFor(() => {
      expect(screen.getByText("查资料")).toBeInTheDocument();
    });
    expect(screen.getByText("执行中")).toBeInTheDocument();
    expect(screen.getByText(/1 个执行中/)).toBeInTheDocument();
  });

  // 环境信息段只留概览 + 跳转：完整清单在右侧栏文件页的「改动」子页，
  // 同一份数据不在浮层里再画一遍
  it("环境信息段给会话口径的累计增删行与跳转，清单不重复渲染", async () => {
    const user = userEvent.setup();
    const onShowFileChanges = vi.fn();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_plan_state") return null;
      if (cmd === "bot_session_file_changes")
        return {
          items: [
            {
              path: "/tmp/a.rs",
              kind: "modify",
              added: 12,
              deleted: 3,
              changes: 1,
              mode: "session",
              drifted: false,
            },
          ],
        };
      return null;
    });
    renderPanel({ onShowFileChanges });
    await waitFor(() => {
      expect(screen.getByText("+12")).toBeInTheDocument();
    });
    expect(screen.getByText("-3")).toBeInTheDocument();
    expect(
      screen.getByTitle("会话口径（累计改动，非 git 差异）"),
    ).toBeInTheDocument();
    expect(screen.queryByText("a.rs")).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: /在右侧栏看改动清单/ }),
    );
    expect(onShowFileChanges).toHaveBeenCalled();
  });
});

describe("计划面板：Esc 层级裁决", () => {
  it("Esc 关闭面板", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderPanel({ onClose });
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("更上层浮层开着时 Esc 让位，不关面板", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderPanel({ onClose, escBlocked: true });
    await user.keyboard("{Escape}");
    expect(onClose).not.toHaveBeenCalled();
  });
});
