// TracePanel 组件测试（P2-a）：mock trace 命令，覆盖摘要头/时间线/diff 着色/回滚。
// invoke mock 模式对齐 ArchivePage.test.tsx（vi.mock("@tauri-apps/api/core")）。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { DiffView } from "./DiffView";
import { TracePanel } from "./TracePanel";
import type { TraceDetail, TraceRow } from "../../lib/trace";

const TRACE: TraceRow = {
  id: 7,
  sessionId: "s1",
  taskId: "t1",
  origin: "manual",
  title: "测试卡",
  status: "done",
  startedAt: Date.now() - 5_000,
  finishedAt: Date.now(),
  turnCount: 3,
  toolCalls: 2,
  filesChanged: 1,
  promptTokens: 100,
  completionTokens: 50,
  error: null,
};

const DETAIL: TraceDetail = {
  ...TRACE,
  spans: [
    {
      id: 1,
      traceId: 7,
      turn: 0,
      toolCallId: "c1",
      name: "read_text_file",
      args: '{"path":"/a/x.py"}',
      result: "文件内容…",
      ok: true,
      errorClass: null,
      durationMs: 12,
      createdAt: Date.now(),
    },
    {
      id: 2,
      traceId: 7,
      turn: 1,
      toolCallId: "c2",
      name: "edit_file",
      args: '{"path":"/a/x.py"}',
      result: null,
      ok: false,
      errorClass: null,
      durationMs: 30,
      createdAt: Date.now(),
    },
  ],
  fileChanges: [
    {
      id: 9,
      traceId: 7,
      spanId: 2,
      path: "/a/x.py",
      kind: "modify",
      added: 3,
      deleted: 1,
      diff: "--- /a/x.py\n+++ /a/x.py\n@@ -1 +1 @@\n-old\n+new",
      truncated: false,
      beforeRef: "snap-1",
      beforeSha: "aaa",
      afterSha: "bbb",
      createdAt: Date.now(),
    },
  ],
};

function mockInvoke() {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "trace_list") return Promise.resolve([TRACE]);
    if (cmd === "trace_detail") return Promise.resolve(DETAIL);
    if (cmd === "file_rollback") return Promise.resolve("已回滚 /a/x.py（恢复到本次修改前）");
    return Promise.reject(new Error(`unexpected command: ${cmd}`));
  });
}

describe("DiffView", () => {
  it("unified diff 按行着色：+/-/hunk 头各归其色", () => {
    const { container } = render(
      <DiffView diff={"--- f\n+++ f\n@@ -1 +1 @@\n-old\n+new\n ctx"} />
    );
    const rows = container.querySelectorAll("pre > div");
    expect(rows).toHaveLength(6);
    expect(rows[0].className).toContain("text-[var(--t5)]"); // --- 文件头
    expect(rows[3].className).toContain("--danger"); // -old
    expect(rows[4].className).toContain("--ok"); // +new
    expect(rows[5].className).toBe(""); // 上下文无着色
  });

  it("truncated 置位时展示截断提示", () => {
    render(<DiffView diff="+a" truncated />);
    expect(screen.getByText(/diff 超长已截断/)).toBeTruthy();
  });
});

describe("TracePanel", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    mockInvoke();
  });

  it("渲染摘要头（轮数/工具/文件/tokens）与工具时间线（含失败标记）", async () => {
    render(<TracePanel taskId="t1" taskTitle="测试卡" onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText(/3 轮 · 2 次工具 · 1 个文件/)).toBeTruthy());
    expect(screen.getByText("read_text_file")).toBeTruthy();
    expect(screen.getByText("edit_file")).toBeTruthy();
    expect(screen.getByText("· 失败")).toBeTruthy();
    expect(screen.getByText(/tokens 100\/50/)).toBeTruthy();
  });

  it("文件修改区展示 ±行与 diff，回滚成功后重载 detail", async () => {
    render(<TracePanel taskId="t1" onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText("+3")).toBeTruthy());
    expect(screen.getByText("−1")).toBeTruthy();
    fireEvent.click(screen.getByText("查看 diff"));
    expect(screen.getByText("-old")).toBeTruthy();
    expect(screen.getByText("+new")).toBeTruthy();
    fireEvent.click(screen.getByText("回滚"));
    await waitFor(() => expect(screen.getByText(/已回滚/)).toBeTruthy());
    expect(invokeMock).toHaveBeenCalledWith("file_rollback", { changeId: 9 });
    // 回滚成功触发 detail 重载
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter((c) => c[0] === "trace_detail").length).toBeGreaterThanOrEqual(2)
    );
  });

  it("回滚被漂移闸拒绝时原文展示原因", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "trace_list") return Promise.resolve([TRACE]);
      if (cmd === "trace_detail") return Promise.resolve(DETAIL);
      if (cmd === "file_rollback")
        return Promise.reject("文件自本次修改后已被改动（指纹不符），拒绝回滚以免吞掉后续修改；请人工核对：/a/x.py");
      return Promise.reject(new Error(`unexpected command: ${cmd}`));
    });
    render(<TracePanel taskId="t1" onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText("回滚")).toBeTruthy());
    fireEvent.click(screen.getByText("回滚"));
    await waitFor(() => expect(screen.getByText(/拒绝回滚以免吞掉后续修改/)).toBeTruthy());
  });

  it("无执行痕迹时空态文案", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "trace_list") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${cmd}`));
    });
    render(<TracePanel taskId="t-empty" onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/还没有执行痕迹/)).toBeTruthy()
    );
  });
});
