import { describe, it, expect, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { TodoCard, TodoCardView } from "./TodoCard";
import type { Task } from "../types";

// —— Tauri mocks ——
// 任务卡虽然本身不直接调 invoke（除复制/删绑定文件外），但子组件 ActorAvatar 会调用
// profile API + listen；mock 掉避免 Profile 初始化抛错。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));
const invokeMock = vi.mocked(invoke);

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn(async () => null) },
}));

// profile 子模块：返回基本用户/机器人资料即可
vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  loadProfile: vi.fn(async () => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
}));

const baseTask: Task = {
  id: "t1",
  title: "默认任务",
  column: "todo",
  order: 0,
};

const dueTask: Task = {
  ...baseTask,
  id: "t2",
  title: "有截止时间",
  due: "2026-08-18T15:30",
};

const completedTask: Task = {
  ...baseTask,
  id: "t3",
  title: "已完成",
  column: "done",
  completedAt: Date.now(),
};

describe("TodoCardView", () => {
  it("默认渲染：标题、DoneCircle、FoldToggle 都可见", () => {
    render(
      <TodoCardView task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.getByText("默认任务")).toBeInTheDocument();
    expect(screen.getByTitle("标记完成")).toBeInTheDocument();
    expect(screen.getByTitle("收起")).toBeInTheDocument();
  });

  it("点击标题进入编辑态：input 显示当前 title，按 Esc 取消恢复", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByText("默认任务"));
    const input = screen.getByDisplayValue("默认任务");
    expect(input).toBeInTheDocument();
    // Esc 取消：保留旧 title，关闭编辑态
    await user.keyboard("{Escape}");
    expect(screen.getByText("默认任务")).toBeInTheDocument();
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it("编辑态输入新标题按 Enter 提交，trim 后非空才回调 onUpdate", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByText("默认任务"));
    const input = screen.getByDisplayValue("默认任务");
    await user.clear(input);
    await user.type(input, "  新标题  ");
    await user.keyboard("{Enter}");
    expect(onUpdate).toHaveBeenCalledWith("t1", { title: "新标题" });
  });

  it("点击 FoldToggle 触发折叠回调：传入 collapsed: true", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByTitle("收起"));
    expect(onUpdate).toHaveBeenCalledWith("t1", { collapsed: true });
  });

  it("点击 DoneCircle 触发完成回调：未完成 → column=done + completedAt + 清 botAssigned", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    const taskWithBot: Task = { ...baseTask, botAssigned: true };
    render(
      <TodoCardView
        task={taskWithBot}
        onUpdate={onUpdate}
        onDelete={vi.fn()}
      />,
    );
    await user.click(screen.getByTitle("标记完成"));
    const call = onUpdate.mock.calls[0];
    expect(call[0]).toBe("t1");
    expect(call[1].column).toBe("done");
    expect(typeof call[1].completedAt).toBe("number");
    expect(call[1].botAssigned).toBeUndefined();
  });

  it("截止日期显示：formatDue 输出「截止 YYYY-MM-DD HH:mm」（2026-09-08 老板加年份）", () => {
    render(
      <TodoCardView task={dueTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.getByText(/截止 2026-08-18 15:30/)).toBeInTheDocument();
  });
});

describe("TodoCard (归档/回收站版)", () => {
  it("归档态：渲染「恢复」按钮，隐藏 DoneCircle 与删除按钮", () => {
    const onUpdate = vi.fn();
    const onDelete = vi.fn();
    render(
      <TodoCard
        task={baseTask}
        archived
        onUpdate={onUpdate}
        onDelete={onDelete}
      />,
    );
    expect(screen.getByText("恢复")).toBeInTheDocument();
    expect(screen.queryByTitle("标记完成")).not.toBeInTheDocument();
    // 标题行右侧的删除按钮（🗑️）在归档态不渲染
    expect(screen.queryByTitle("删除任务")).not.toBeInTheDocument();
  });

  it("回收站态：渲染「恢复」+「彻底删除」按钮", () => {
    const onUpdate = vi.fn();
    const onDelete = vi.fn();
    render(
      <TodoCard
        task={baseTask}
        trashed
        onUpdate={onUpdate}
        onDelete={onDelete}
      />,
    );
    expect(screen.getByText("恢复")).toBeInTheDocument();
    expect(screen.getByText(/彻底删除/)).toBeInTheDocument();
  });

  it("完成态：归档态显示完成时间（formatCompletedAt 在 due 区域下方，2026-09-08 老板）", () => {
    render(
      <TodoCard
        task={completedTask}
        archived
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(
      screen.getByText(/完成 \d{4}-\d{2}-\d{2} \d{2}:\d{2}/),
    ).toBeInTheDocument();
  });
});

// 让 dnd-kit useDraggable 在 jsdom 下不报错；TodoCard 直接调用 useDraggable 但不影响按钮交互
describe("TodoCard useDraggable 集成", () => {
  it("渲染 TodoCard 不会抛错（useDraggable 在 jsdom 中可工作）", () => {
    expect(() =>
      render(
        <TodoCard task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
      ),
    ).not.toThrow();
    // 验证整体结构存在
    expect(screen.getByText("默认任务")).toBeInTheDocument();
  });
});

// 兜底：测试用具，sanity check jsdom + @testing-library/jest-dom
describe("TodoCard 测试环境", () => {
  it("toBeInTheDocument matcher 可用", () => {
    const { container } = render(
      <TodoCardView task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(within(container).getByText("默认任务")).toBeInTheDocument();
  });
});

// —— 多文件绑定：chip 列表 / 单独移除 / 超 5 折叠 / 上限 / 文件与文件夹不互斥 ——
describe("TodoCardView 多文件绑定", () => {
  const multiTask: Task = {
    ...baseTask,
    files: [
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/b.docx", isDir: false },
      { path: "/docs/c.txt", isDir: false },
    ],
  };

  it("多 chip 列表渲染：每个文件一行（图标 + basename + 移除按钮）", () => {
    render(
      <TodoCardView task={multiTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.getByText("a.pdf")).toBeInTheDocument();
    expect(screen.getByText("b.docx")).toBeInTheDocument();
    expect(screen.getByText("c.txt")).toBeInTheDocument();
    expect(screen.getAllByTitle("移除该文件")).toHaveLength(3);
  });

  it("chip × 单独移除：files 去掉该条", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={multiTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getAllByTitle("移除该文件")[0]);
    const call = onUpdate.mock.calls[0];
    expect(call[0]).toBe("t1");
    expect(call[1].files).toEqual([
      { path: "/docs/b.docx", isDir: false },
      { path: "/docs/c.txt", isDir: false },
    ]);
  });

  it("超过 5 个折叠为「还有 N 个」，点击展开/收起", async () => {
    const user = userEvent.setup();
    const seven: Task = {
      ...baseTask,
      files: Array.from({ length: 7 }, (_, i) => ({
        path: `/f/${i}.txt`,
        isDir: false,
      })),
    };
    render(<TodoCardView task={seven} onUpdate={vi.fn()} onDelete={vi.fn()} />);
    // 默认只显示前 5 个
    expect(screen.getByText("4.txt")).toBeInTheDocument();
    expect(screen.queryByText("5.txt")).not.toBeInTheDocument();
    expect(screen.getByText("还有 2 个")).toBeInTheDocument();
    await user.click(screen.getByText("还有 2 个"));
    expect(screen.getByText("6.txt")).toBeInTheDocument();
    expect(screen.getByText("收起")).toBeInTheDocument();
  });

  it("文件与文件夹不互斥：已绑文件夹时仍显示「＋」，但不再显示「绑定文件夹」", () => {
    const dirTask: Task = {
      ...baseTask,
      files: [{ path: "/some/dir", isDir: true }],
    };
    render(
      <TodoCardView task={dirTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.getByText("dir")).toBeInTheDocument();
    expect(screen.getByTitle("继续绑定文件")).toBeInTheDocument();
    expect(screen.queryByTitle("绑定文件夹")).not.toBeInTheDocument();
  });

  it("已绑文件时显示「绑定文件夹」，pickFolder 追加文件夹而不是替换文件", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    const oneFile: Task = {
      ...baseTask,
      files: [{ path: "/docs/a.pdf", isDir: false }],
    };
    vi.mocked(open).mockResolvedValueOnce("/some/dir" as never);
    render(
      <TodoCardView task={oneFile} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByTitle("绑定文件夹"));
    expect(vi.mocked(open)).toHaveBeenCalledWith({ directory: true });
    const call = onUpdate.mock.calls[0];
    // 文件夹追加在后，已绑文件保留
    expect(call[1].files).toEqual([
      { path: "/docs/a.pdf", isDir: false },
      { path: "/some/dir", isDir: true },
    ]);
  });

  it("pickFile 多选追加：merge 去重保序 + bind_files 取 isDir", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const { invoke } = await import("@tauri-apps/api/core");
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    const oneFile: Task = {
      ...baseTask,
      files: [{ path: "/docs/a.pdf", isDir: false }],
    };
    vi.mocked(open).mockResolvedValueOnce([
      "/docs/a.pdf",
      "/docs/new.txt",
    ] as never);
    vi.mocked(invoke).mockResolvedValueOnce([
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/new.txt", isDir: false },
    ] as never);
    render(
      <TodoCardView task={oneFile} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByTitle("继续绑定文件"));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("bind_files", {
      paths: ["/docs/a.pdf", "/docs/new.txt"],
    });
    const call = onUpdate.mock.calls[0];
    // 重复的 a.pdf 不叠加，new.txt 追加在后
    expect(call[1].files).toEqual([
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/new.txt", isDir: false },
    ]);
  });

  it("pickFile 超上限：截断到 10 并弹提示", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const { invoke } = await import("@tauri-apps/api/core");
    const alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    const nine: Task = {
      ...baseTask,
      files: Array.from({ length: 9 }, (_, i) => ({
        path: `/f/${i}.txt`,
        isDir: false,
      })),
    };
    const added = [
      { path: "/f/9.txt", isDir: false },
      { path: "/f/10.txt", isDir: false },
    ];
    vi.mocked(open).mockResolvedValueOnce(added.map((f) => f.path) as never);
    vi.mocked(invoke).mockResolvedValueOnce(added as never);
    render(<TodoCardView task={nine} onUpdate={onUpdate} onDelete={vi.fn()} />);
    // 9 个文件时仍有「＋」（< 10）
    await user.click(screen.getByTitle("继续绑定文件"));
    const call = onUpdate.mock.calls[0];
    expect(call[1].files).toHaveLength(10);
    expect(alertSpy).toHaveBeenCalled();
    alertSpy.mockRestore();
  });

  it("pickFile 对话框挂起期间 task.files 被并发改动：合并基线取最新，两批文件都不丢", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const { invoke } = await import("@tauri-apps/api/core");
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    // 挂起 open：点击「＋」后停在对话框，模拟用户选文件期间别处（另一窗口/机器人）动了绑定
    let releaseOpen!: (v: string[]) => void;
    vi.mocked(open).mockImplementationOnce(
      () =>
        new Promise<string[]>((resolve) => {
          releaseOpen = resolve;
        }) as never,
    );
    vi.mocked(invoke).mockResolvedValueOnce([
      { path: "/docs/new.txt", isDir: false },
    ] as never);
    const before: Task = {
      ...baseTask,
      files: [{ path: "/docs/a.pdf", isDir: false }],
    };
    const { rerender } = render(
      <TodoCardView task={before} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByTitle("继续绑定文件"));
    // 对话框挂起期间：并发进来一个绑定（task.files 多了 concurrent.txt）
    rerender(
      <TodoCardView
        task={{
          ...before,
          files: [
            { path: "/docs/a.pdf", isDir: false },
            { path: "/docs/concurrent.txt", isDir: false },
          ],
        }}
        onUpdate={onUpdate}
        onDelete={vi.fn()}
      />,
    );
    // 放行对话框 → bind_files → 合并
    await act(async () => {
      releaseOpen(["/docs/new.txt"]);
    });
    const call = onUpdate.mock.calls[0];
    expect(call[0]).toBe("t1");
    // 点击时的快照（只有 a.pdf）不得当基线：并发批 concurrent.txt 必须保留
    expect(call[1].files).toEqual([
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/concurrent.txt", isDir: false },
      { path: "/docs/new.txt", isDir: false },
    ]);
  });

  it("pickFolder 对话框挂起期间 task.files 被并发改动：isDir 复查与合并都取最新，两批文件都不丢", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    let releaseOpen!: (v: string) => void;
    vi.mocked(open).mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          releaseOpen = resolve;
        }) as never,
    );
    const before: Task = {
      ...baseTask,
      files: [{ path: "/docs/a.pdf", isDir: false }],
    };
    const { rerender } = render(
      <TodoCardView task={before} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByTitle("绑定文件夹"));
    // 对话框挂起期间：并发绑进一个文件（文件夹与文件不互斥，不拦）
    rerender(
      <TodoCardView
        task={{
          ...before,
          files: [
            { path: "/docs/a.pdf", isDir: false },
            { path: "/docs/concurrent.txt", isDir: false },
          ],
        }}
        onUpdate={onUpdate}
        onDelete={vi.fn()}
      />,
    );
    await act(async () => {
      releaseOpen("/some/dir");
    });
    const call = onUpdate.mock.calls[0];
    // 陈旧基线（只有 a.pdf）会丢掉 concurrent.txt；文件夹追加在后
    expect(call[1].files).toEqual([
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/concurrent.txt", isDir: false },
      { path: "/some/dir", isDir: true },
    ]);
  });

  it("chip 内「复制」字样：逐文件调 copy_file_with_title（2026-08-26 起替代 📋 按钮）", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const user = userEvent.setup();
    render(
      <TodoCardView task={multiTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    const copies = screen.getAllByTitle("复制文件+标题");
    expect(copies).toHaveLength(3);
    await user.click(copies[1]);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("copy_file_with_title", {
      path: "/docs/b.docx",
      title: "默认任务",
    });
  });

  it("点绑定文件名直接打开对应文件（2026-08-26 起不再有 📂 多选列表；走 Rust open_file_path，绕 opener scope 限 $HOME 导致的 Windows 静默失败）", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const user = userEvent.setup();
    render(
      <TodoCardView task={multiTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByText("b.docx"));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("open_file_path", {
      path: "/docs/b.docx",
    });
  });
});

// —— 归属头像规则：定时设置后一直机器人头像；执行中机器人头像；执行完恢复用户头像 ——
describe("TodoCardView 归属头像", () => {
  it("无定时未交机器人 → 用户头像（profile mock 无图时显示姓名首字）", () => {
    render(
      <TodoCardView task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.queryByAltText("Bot")).not.toBeInTheDocument();
    expect(screen.getByTitle("Test")).toBeInTheDocument();
  });

  it("botAssigned=true（机器人干活中）→ 机器人头像", () => {
    render(
      <TodoCardView
        task={{ ...baseTask, botAssigned: true }}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByAltText("Bot")).toBeInTheDocument();
    expect(screen.queryByTitle("Test")).not.toBeInTheDocument();
  });

  it("机器人干完活（botAssigned 清除）→ 恢复用户头像", () => {
    // botAssigned: undefined 模拟执行结束后的落库状态（后端 set_bot_assigned(false)）
    render(
      <TodoCardView
        task={{ ...baseTask, botAssigned: undefined }}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.queryByAltText("Bot")).not.toBeInTheDocument();
    expect(screen.getByTitle("Test")).toBeInTheDocument();
  });
});

// —— 子任务：点击文本内联编辑 + 全文显示不截断 ——
describe("TodoCardView 子任务", () => {
  const subTask: Task = {
    ...baseTask,
    subtasks: [
      { id: "s1", text: "第一步子任务", done: false },
      { id: "s2", text: "第二步子任务", done: true },
    ],
  };

  it("子任务文本完整显示：不带 truncate 单行截断", () => {
    render(
      <TodoCardView task={subTask} onUpdate={vi.fn()} onDelete={vi.fn()} />,
    );
    const el = screen.getByText("第一步子任务");
    expect(el.className).not.toContain("truncate");
    expect(el.className).toContain("whitespace-pre-wrap");
  });

  it("点击子任务文本进入编辑，Enter 提交更新该条", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={subTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    await user.click(screen.getByText("第一步子任务"));
    const input = screen.getByDisplayValue("第一步子任务");
    await user.clear(input);
    await user.type(input, "改后的子任务{Enter}");
    const call = onUpdate.mock.calls[0];
    expect(call[0]).toBe("t1");
    expect(call[1].subtasks).toEqual([
      { id: "s1", text: "改后的子任务", done: false },
      { id: "s2", text: "第二步子任务", done: true },
    ]);
  });

  it("子任务编辑：Escape 取消不写库；空提交保留原文", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(
      <TodoCardView task={subTask} onUpdate={onUpdate} onDelete={vi.fn()} />,
    );
    // Escape 取消
    await user.click(screen.getByText("第一步子任务"));
    await user.keyboard("改成别的{Escape}");
    expect(onUpdate).not.toHaveBeenCalled();
    // 空提交保留原文
    await user.click(screen.getByText("第一步子任务"));
    const input = screen.getByDisplayValue("第一步子任务");
    await user.clear(input);
    await user.keyboard("{Enter}");
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it("归档态子任务只读：点击不进入编辑", async () => {
    const user = userEvent.setup();
    render(
      <TodoCard
        task={subTask}
        archived
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    await user.click(screen.getByText("第一步子任务"));
    expect(screen.queryByDisplayValue("第一步子任务")).not.toBeInTheDocument();
  });
  it("永久删除带绑定文件时逐个删文件并传本卡 id（后端据此判归属、拒删他人引用的文件）", async () => {
    const user = userEvent.setup();
    invokeMock.mockClear();
    const onDelete = vi.fn();
    render(
      <TodoCard
        task={{
          ...baseTask,
          files: [{ path: "/tmp/a.pdf", isDir: false }],
        }}
        trashed
        onUpdate={vi.fn()}
        onDelete={onDelete}
      />,
    );
    await user.click(screen.getByText("彻底删除"));
    await user.click(screen.getByText("全部删除").closest("button")!);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("delete_bound_file", {
        path: "/tmp/a.pdf",
        taskId: "t1",
      }),
    );
    expect(onDelete).toHaveBeenCalledWith("t1");
  });

  it("彻底删除：某个文件被拒（他人引用）不中断整批，卡片保留且报出每个失败项", async () => {
    const user = userEvent.setup();
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      const p = (args as { path?: string } | undefined)?.path ?? "";
      if (cmd === "delete_bound_file" && p.includes("b.pdf")) {
        throw new Error("该文件仍被 任务「另一张卡」引用");
      }
      return null;
    });
    const onDelete = vi.fn();
    const alerts: string[] = [];
    vi.stubGlobal("alert", (msg: string) => alerts.push(msg));
    render(
      <TodoCard
        task={{
          ...baseTask,
          files: [
            { path: "/tmp/a.pdf", isDir: false },
            { path: "/tmp/b.pdf", isDir: false },
          ],
        }}
        trashed
        onUpdate={vi.fn()}
        onDelete={onDelete}
      />,
    );
    await user.click(screen.getByText("彻底删除"));
    await user.click(screen.getByText("全部删除").closest("button")!);
    await waitFor(() => expect(alerts).toHaveLength(1));
    // 两个文件都尝试过（不因第一个失败就跳过第二个）
    const deleteCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "delete_bound_file",
    );
    expect(deleteCalls).toHaveLength(2);
    expect(alerts[0]).toContain("已移入回收站 1/2");
    expect(alerts[0]).toContain("b.pdf");
    // 卡不删：还有文件没处理完
    expect(onDelete).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
