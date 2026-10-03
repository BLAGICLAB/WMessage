// MemoryPanel 测试（U14）：列表渲染 / 统计与降级横幅 / 搜索防抖 /
// 编辑保存 / 删除确认（含自进化条目文案）/ 取消删除。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryPanel, type MemItemView, type MemStats } from "./MemoryPanel";

const mocks = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.invokeMock.mockImplementation(async () => null);
  window.confirm = vi.fn(() => true);
});

async function flush() {
  await act(async () => {});
}

function sampleItem(over: Partial<MemItemView> = {}): MemItemView {
  return {
    id: "m1",
    kind: "preference",
    content: "喜欢简洁的回复风格",
    tags: ["style"],
    importance: 4,
    source: "user_stated",
    createdAt: 1_760_000_000_000,
    updatedAt: 1_760_000_000_000,
    accessCount: 2,
    lastAccessedAt: null,
    hasEmbedding: true,
    ...over,
  };
}

function sampleStats(over: Partial<MemStats> = {}): MemStats {
  return {
    total: 3,
    byKind: [
      ["preference", 2],
      ["lesson", 1],
    ],
    bySource: [["user_stated", 3]],
    withEmbedding: 1,
    capacity: 500,
    embedOk: true,
    embedError: null,
    ...over,
  };
}

/** 默认数据源：列表两条 + 统计 */
function stubData(items: MemItemView[] = [sampleItem()], stats: MemStats = sampleStats()) {
  mocks.invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "mem_list") return items;
    if (cmd === "mem_stats") return stats;
    if (cmd === "mem_update" || cmd === "mem_delete") return null;
    void args;
    return null;
  });
}

describe("MemoryPanel", () => {
  it("空列表：显示空态引导", async () => {
    stubData([], sampleStats({ total: 0, withEmbedding: 0, byKind: [], bySource: [] }));
    render(<MemoryPanel />);
    expect(await screen.findByText(/还没有匹配的记忆/)).toBeInTheDocument();
  });

  it("列表渲染：内容截断 title / 类型与来源徽标 / 重要度 / 时间与访问次数", async () => {
    stubData([
      sampleItem(),
      sampleItem({
        id: "m2",
        kind: "lesson",
        content: "执行任务前先检查文件是否存在",
        source: "model_inferred",
        importance: 3,
        hasEmbedding: false,
        accessCount: 0,
      }),
    ]);
    render(<MemoryPanel />);
    expect(await screen.findByText("喜欢简洁的回复风格")).toBeInTheDocument();
    expect(screen.getByText("执行任务前先检查文件是否存在")).toBeInTheDocument();
    // 类型徽标 + 来源徽标 + 重要度
    expect(screen.getAllByText("偏好").length).toBeGreaterThan(0);
    expect(screen.getAllByText("教训").length).toBeGreaterThan(0);
    expect(screen.getByText("推断")).toBeInTheDocument();
    expect(screen.getByText("★4")).toBeInTheDocument();
    // 无向量条目徽标半透明 + title 标注（「教训」与筛选 chip 重名，走 title 定位）
    const lessonBadge = screen.getByTitle("无向量（关键词模式检索）");
    expect(lessonBadge.className).toContain("opacity-50");
    // 被想起次数行内直显；0 次不显示
    expect(screen.getByText("想起 2")).toBeInTheDocument();
    expect(screen.queryByText("想起 0")).toBeNull();
  });

  it("统计行与嵌入降级横幅：embedOk=false 显示原因；正常态不显示", async () => {
    stubData([sampleItem()], sampleStats({ embedOk: false, embedError: "模型目录缺失" }));
    render(<MemoryPanel />);
    expect(await screen.findByText(/3\/500 条 · 向量覆盖 1\/3/)).toBeInTheDocument();
    expect(screen.getByText(/语义嵌入不可用，已降级为关键词检索：模型目录缺失/)).toBeInTheDocument();
  });

  it("搜索防抖：停顿 300ms 后才带 query 打 mem_list", async () => {
    const user = userEvent.setup();
    stubData();
    render(<MemoryPanel />);
    await screen.findByText("喜欢简洁的回复风格");
    const listCallsBefore = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "mem_list" && c[1]?.query,
    ).length;
    await user.type(screen.getByLabelText("搜索记忆"), "风格");
    // 防抖窗口内不触发
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "mem_list" && c[1]?.query).length,
    ).toBe(listCallsBefore);
    // 停顿超过 300ms → 带 query 重查
    await act(async () => {
      await new Promise((r) => setTimeout(r, 400));
    });
    const last = mocks.invokeMock.mock.calls.filter((c) => c[0] === "mem_list").pop();
    expect(last?.[1]?.query).toBe("风格");
  });

  it("编辑保存：铅笔进编辑态 → 改内容/重要度/类型 → mem_update 参数断言 + 刷新", async () => {
    const user = userEvent.setup();
    stubData();
    render(<MemoryPanel />);
    await screen.findByText("喜欢简洁的回复风格");
    await user.click(screen.getByRole("button", { name: "编辑记忆" }));
    const content = screen.getByLabelText("编辑记忆内容");
    // textarea 受控编辑
    await user.clear(content);
    await user.type(content, "喜欢极简的回复风格");
    await user.selectOptions(screen.getByLabelText("记忆重要度"), "5");
    await user.selectOptions(screen.getByLabelText("记忆类型"), "preference");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await flush();
    const call = mocks.invokeMock.mock.calls.find((c) => c[0] === "mem_update");
    expect(call?.[1]).toMatchObject({
      id: "m1",
      content: "喜欢极简的回复风格",
      importance: 5,
      kind: "preference",
    });
    // 保存后退出编辑态
    expect(screen.queryByLabelText("编辑记忆内容")).toBeNull();
  });

  it("删除：确认后调 mem_delete；普通条目与自进化条目确认文案不同", async () => {
    const user = userEvent.setup();
    const confirmMock = vi.fn((_msg?: string) => true);
    window.confirm = confirmMock;
    stubData([
      sampleItem(),
      sampleItem({ id: "m2", tags: ["evo:abc"], content: "提案教训" }),
    ]);
    render(<MemoryPanel />);
    await screen.findByText("喜欢简洁的回复风格");
    // 普通条目
    const delBtns = screen.getAllByRole("button", { name: "删除记忆" });
    await user.click(delBtns[0]);
    expect(confirmMock.mock.calls[0]?.[0]).not.toContain("自进化");
    await flush();
    expect(mocks.invokeMock.mock.calls.some((c) => c[0] === "mem_delete" && c[1]?.id === "m1")).toBe(true);
    // 自进化条目：确认文案点名影响
    await user.click(delBtns[1]);
    expect(confirmMock.mock.calls[1]?.[0]).toContain("自进化");
  });

  it("取消删除：confirm 返回 false 不调 mem_delete", async () => {
    const user = userEvent.setup();
    window.confirm = vi.fn(() => false);
    stubData();
    render(<MemoryPanel />);
    await screen.findByText("喜欢简洁的回复风格");
    await user.click(screen.getByRole("button", { name: "删除记忆" }));
    await flush();
    expect(mocks.invokeMock.mock.calls.some((c) => c[0] === "mem_delete")).toBe(false);
  });
});
