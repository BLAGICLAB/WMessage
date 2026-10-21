import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ActivityPage } from "./index";
import type { TraceRow } from "../../lib/trace";

// —— Tauri mocks：invoke 按 command 名路由（trace_list）——
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

function trace(partial: Partial<TraceRow> & { id: number }): TraceRow {
  return {
    sessionId: "s1",
    origin: "manual",
    title: "周报生成",
    status: "done",
    startedAt: 1_700_000_000_000,
    finishedAt: 1_700_000_060_000,
    turnCount: 2,
    toolCalls: 1,
    filesChanged: 0,
    promptTokens: 0,
    completionTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    ...partial,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "trace_list") return [trace({ id: 1, origin: "chat", title: null })];
    return null;
  });
});

describe("ActivityPage 来源标签", () => {
  it("聊天回合的来源显示中文标签，不露英文原值", async () => {
    render(<ActivityPage onOpenChat={vi.fn()} />);
    // 列表行里的来源标签（页首还有同名的筛选 chip，按 list 作用域区分）
    const list = await screen.findByRole("list");
    expect(within(list).getByText("聊天")).toBeInTheDocument();
    expect(within(list).queryByText("chat")).not.toBeInTheDocument();
  });
});

describe("ActivityPage 来源筛选", () => {
  it("切到某个来源走后端 origin 过滤（不在前端筛）", async () => {
    const user = userEvent.setup();
    render(<ActivityPage onOpenChat={vi.fn()} />);
    await user.click(await screen.findByRole("button", { name: "聊天" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("trace_list", {
        origin: "chat",
        limit: 50,
      });
    });
  });

  it("默认看全部：不限来源", async () => {
    render(<ActivityPage onOpenChat={vi.fn()} />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("trace_list", { limit: 50 });
    });
  });
});
