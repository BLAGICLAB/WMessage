import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ContextRail, type SubagentRun } from "./ContextRail";
import type { Task } from "../../types";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openTargetMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("../../lib/openTarget", async (importActual) => {
  const actual = await importActual<typeof import("../../lib/openTarget")>();
  return { ...actual, openTarget: mocks.openTargetMock };
});

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
  mocks.openTargetMock.mockReset();
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

// 文件预览的工作面：点行就地看内容，不跳系统默认程序。预览路径是受控的——
// 对话正文里的路径链接也要能打开它，那些点击不在侧栏内部。
describe("ContextRail 文件预览", () => {
  const file = { path: "/Users/x/调研.md", added: 3, deleted: 1 };
  function previewOf(path: string) {
    return {
      kind: "text",
      path,
      content: "# 上海博士后政策\n正文一段",
      truncated: false,
      totalBytes: 20,
      lines: 2,
    };
  }

  it("点文件行：整栏切成预览并报给 onPreviewPath，不调起系统默认程序", async () => {
    const user = userEvent.setup();
    const onPreviewPath = vi.fn();
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_preview_text" ? previewOf(file.path) : null,
    );
    renderRail({
      tab: "files",
      fileChanges: [file],
      previewPath: null,
      onPreviewPath,
    });
    await user.click(screen.getByTitle(/点开预览内容/));
    expect(onPreviewPath).toHaveBeenCalledWith(file.path);
    expect(mocks.openTargetMock).not.toHaveBeenCalled();
  });

  it("预览路径受父级控制：正文链接打开的路径即使不在改动列表里也能预览", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_preview_text" ? previewOf("/Users/x/别的.md") : null,
    );
    renderRail({
      tab: "files",
      fileChanges: [file],
      previewPath: "/Users/x/别的.md",
      onPreviewPath: vi.fn(),
    });
    expect(await screen.findByLabelText("文件内容")).toHaveTextContent(
      "上海博士后政策",
    );
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_preview_text", {
      path: "/Users/x/别的.md",
    });
    // 不在本会话改动记录里 → 没有 ±行数可显示，那一行就不出现
    expect(screen.queryByText(/^\+/)).toBeNull();
  });

  it("改动记录里命中的路径：预览头部带 ±行数", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_preview_text" ? previewOf(file.path) : null,
    );
    renderRail({
      tab: "files",
      fileChanges: [file],
      previewPath: file.path,
      onPreviewPath: vi.fn(),
    });
    expect(await screen.findByText("+3")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
  });

  it("预览里的「用系统默认程序打开」按钮仍是出口（栏内看 ≠ 没有外部编辑器）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_preview_text" ? previewOf(file.path) : null,
    );
    renderRail({
      tab: "files",
      fileChanges: [file],
      previewPath: file.path,
      onPreviewPath: vi.fn(),
    });
    await user.click(
      await screen.findByLabelText("用系统默认程序打开 调研.md"),
    );
    expect(mocks.openTargetMock).toHaveBeenCalledWith(file.path);
  });

  it("返回文件列表：回到列表而不是空白", async () => {
    const user = userEvent.setup();
    const onPreviewPath = vi.fn();
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_preview_text" ? previewOf(file.path) : null,
    );
    renderRail({
      tab: "files",
      fileChanges: [file],
      previewPath: file.path,
      onPreviewPath,
    });
    await user.click(await screen.findByText("返回文件列表"));
    expect(onPreviewPath).toHaveBeenCalledWith(null);
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

// 类别切换钮的底色过渡必须常驻在元素上。
// nm-inset 是随选中态增删的类，过渡跟着类走就变成「进不补间、退瞬切」：
// 选中时底色淡入、取消选中时底色瞬间消失，hover 底色也是硬切。
describe("ContextRail 类别切换钮的过渡", () => {
  it("选中与未选中两个态都带过渡类（写在常驻段，不是跟着 nm-inset 增删）", () => {
    renderRail();
    // 默认选中「任务」页，「文件」页是未选中态
    const selected = screen.getByRole("button", { name: "任务" });
    const idle = screen.getByRole("button", { name: "文件" });
    expect(selected.getAttribute("aria-pressed")).toBe("true");
    expect(idle.getAttribute("aria-pressed")).toBe("false");
    expect(selected.className).toContain("nm-inset");
    expect(selected.className).toContain("transition-colors");
    expect(idle.className).not.toContain("nm-inset");
    // 未选中态也必须有：撤掉 nm-inset 后过渡不能跟着消失
    expect(idle.className).toContain("transition-colors");
  });

  it("过渡声明不在 nm-inset 分支里（否则退场就是瞬切）", () => {
    const src = readFileSync("src/components/ChatPanel/ContextRail.tsx", "utf-8");
    const branch = src.match(/\?\s*"nm-inset[^"]*"/)?.[0] ?? "";
    expect(branch).not.toContain("transition");
  });
});
