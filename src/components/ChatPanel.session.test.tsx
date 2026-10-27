import { describe, it, expect, vi, beforeEach } from "vitest";
import { chatDefaultInvoke } from "../test/chatTestMocks";
import { render, screen, waitFor, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
import * as streamState from "./ChatPanel/streamState";
import { stubScrollNoop } from "../test/scrollNoop";

// ChatPanel 自动滚动依赖 jsdom 未实现的 scrollTo/scrollIntoView（scoped noop）
stubScrollNoop();

/** 会话标题在两处渲染：常驻侧栏（导航）与聊天头部（上下文指示）。
 *  用例要定位时按所在区域限定作用域，否则 findByText 会命中两处报歧义。 */
const rail = () =>
  within(screen.getByRole("complementary", { name: "会话列表" }));
/** 侧栏内按标题找会话行（切会话的点击目标） */
const railSession = (title: string | RegExp) => rail().getByText(title);
/** 头部标题：确认「当前在看哪一段对话」 */
const currentTitle = () => screen.getByRole("heading", { level: 2 });

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const emitMock = vi.fn();
  // 捕获 ChatPanel 注册的窗口拖放回调（拖文件进聊天区 → 附件），供用例手动触发
  const dragDropHandlers: Array<
    (e: { payload: Record<string, unknown> }) => unknown
  > = [];
  // 按事件名捕获 listen 回调（chat-open-session 手动触发用）
  const listeners: Record<
    string,
    Array<(e: { payload: Record<string, unknown> }) => void>
  > = {};
  const askMock = vi.fn();
  return { invokeMock, listenMock, emitMock, dragDropHandlers, listeners, askMock };
});

mocks.invokeMock.mockImplementation(chatDefaultInvoke);
mocks.listenMock.mockImplementation(async () => () => {});
mocks.emitMock.mockImplementation(async () => {});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: mocks.listenMock,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onDragDropEvent: async (
      cb: (e: { payload: Record<string, unknown> }) => unknown,
    ) => {
      mocks.dragDropHandlers.push(cb);
      return () => {};
    },
    scaleFactor: async () => 1,
  }),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: (...args: unknown[]) => mocks.askMock(...args),
}));

// navigator.clipboard.writeText
const writeTextMock = vi.fn(async () => {});
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: writeTextMock },
});

const defaultProps = {
  enabled: true,
  // 右侧上下文栏的任务数据与改写回调（App 侧接 updateTask），
  // 这里只验 ChatPanel 内部的传递，不复现 App 的接线
  tasks: [],
  onUpdateTask: () => {},
};

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
  writeTextMock.mockClear();
  mocks.dragDropHandlers.length = 0;
  for (const k of Object.keys(mocks.listeners)) delete mocks.listeners[k];
  // 侧栏/水位档位等视图态持久化在 localStorage，jsdom 同一文件内跨用例共享——
  // 不清会让「默认收起」这类用例被前一个用例写下的展开态污染
  localStorage.clear();
  // 流式在途态在模块 store 里（跨面板挂载存活），同样跨用例共享——
  // 不复位会让上一例的「在途」改写这一例的渲染（占位文案 / 停止键）
  streamState.resetAll();
});
describe("ChatPanel", () => {
  // 任务执行聊天化：chat-open-session 跳转/排队

  it("chat-open-session 非 busy：直接切换到执行会话并加载其历史", async () => {
    // 前面的用例会把 listenMock 恢复成不记录的默认实现，这里显式重设
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "默认会话" }];
        if (cmd === "bot_history_load")
          return args?.sessionId === "s-exec"
            ? [
                {
                  role: "user",
                  content: "[任务卡执行]\nid=t1\n标题：写报告",
                  refsJson: null,
                  thinking: null,
                  toolsJson: null,
                },
              ]
            : [];
        return null;
      },
    );
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: {
            sessionId: "s-exec",
            taskId: "t1",
            title: "📋 任务：写报告",
            origin: "manual",
          },
        });
      }
    });
    // 切到执行会话：加载其历史，任务块渲染出来
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-exec",
      }),
    );
    expect(await screen.findByText(/\[任务卡执行\]/)).toBeInTheDocument();
    // 会话列表已含新会话
    expect(railSession(/📋 任务：写报告/)).toBeInTheDocument();
    expect(currentTitle()).toHaveTextContent("📋 任务：写报告");
  });

  it("chat-open-session 迟到（execute-task 已收尾）：失败任务不自动跳转让用户决定；成功任务切过去只读查看", async () => {
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "默认会话" }];
        if (cmd === "bot_execute_task") {
          if (args?.taskId === "t-fail")
            throw {
              code: "TASK_INVALID_STATE",
              message: "任务失败",
            } as unknown;
          return null;
        }
        if (cmd === "bot_history_load")
          return args?.sessionId === "s-done"
            ? [
                {
                  role: "user",
                  content: "[任务卡执行]\nid=t-done\n标题：已完成",
                  refsJson: null,
                  thinking: null,
                  toolsJson: null,
                },
              ]
            : [];
        return null;
      },
    );
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    // 两张卡都收尾（一成一败）——收尾顺序在迟到事件之前
    await act(async () => {
      for (const cb of mocks.listeners["execute-task"] ?? []) {
        cb({ payload: { id: "t-done", title: "成功任务" } });
        cb({ payload: { id: "t-fail", title: "失败任务" } });
      }
    });
    await act(async () => {});
    // 迟到的失败任务 open-session：不自动切（历史不加载，用户自行决定是否重试/查看）
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: { sessionId: "s-fail", taskId: "t-fail", title: "失败任务" },
        });
      }
    });
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_history_load", {
      sessionId: "s-fail",
    });
    // 迟到的成功任务 open-session：切过去查看最终历史
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: { sessionId: "s-done", taskId: "t-done", title: "成功任务" },
        });
      }
    });
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-done",
      }),
    );
    expect(await screen.findByText(/\[任务卡执行\]/)).toBeInTheDocument();
  });

  it("chat-open-session busy：跳转排队，当前轮结束后出现「查看执行对话」按钮，点击切换", async () => {
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    const user = userEvent.setup();
    let releaseChat: (v: {
      text: string;
      taskRefs: never[];
    }) => void = () => {};
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat")
        return new Promise((r) => {
          releaseChat = r as typeof releaseChat;
        });
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // busy 中收到 chat-open-session → 不切换、不读执行会话历史
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: {
            sessionId: "s-exec",
            taskId: "t1",
            title: "📋 任务：写报告",
            origin: "scheduled",
          },
        });
      }
    });
    expect(railSession("默认会话")).toBeInTheDocument();
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_history_load", {
      sessionId: "s-exec",
    });
    // 当前轮结束 → 排队跳转兑现为提示 + 按钮
    await act(async () => {
      releaseChat({ text: "回复", taskRefs: [] });
    });
    expect(await screen.findByText(/已在新会话执行/)).toBeInTheDocument();
    const btn = await screen.findByText("查看执行对话");
    await user.click(btn);
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-exec",
      }),
    );
  });

  // ── 双命令：🧠 模型下拉 ──

  it("右侧栏文件工作面：点文件看本会话的改动，能切全文、能返回列表", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: { path?: string }) => {
        // 回合不收尾（bot_chat 悬着）：文件改动是流式回合里才有的事件
        if (cmd === "bot_chat") return new Promise(() => {});
        // 清单与详情按后端口径给（累计 ±）
        if (cmd === "bot_session_file_changes") {
          return args?.path
            ? {
                items: [],
                detail: {
                  path: "/tmp/报告.md",
                  kind: "modify",
                  mode: "session",
                  added: 3,
                  deleted: 1,
                  changes: 1,
                  truncated: false,
                  drifted: false,
                  diff:
                    "--- /tmp/报告.md\n+++ /tmp/报告.md\n@@ -1 +1 @@\n-旧标题\n+新标题",
                  perChange: [],
                  note: null,
                },
              }
            : {
                items: [
                  {
                    path: "/tmp/报告.md",
                    kind: "modify",
                    added: 3,
                    deleted: 1,
                    changes: 1,
                    mode: "session",
                    drifted: false,
                    note: null,
                  },
                ],
                detail: null,
              };
        }
        if (cmd === "bot_preview_text")
          return {
            kind: "text",
            path: "/tmp/报告.md",
            content: "# 标题\n\n正文一段",
            truncated: false,
            totalBytes: 14,
            lines: 3,
          };
        return chatDefaultInvoke(cmd);
      },
    );
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      ((mocks.listeners[event] ??= []) as Array<typeof cb>).push(
        cb as (e: { payload: Record<string, unknown> }) => void,
      );
      return () => {};
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.type(screen.getByPlaceholderText(/和机器人说点什么/), "改文件");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-file-changed"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            path: "/tmp/报告.md",
            kind: "modify",
            added: 3,
            deleted: 1,
          },
        });
      }
    });
    // 打开右侧栏 → 文件工作面
    await user.click(screen.getByRole("button", { name: "展开右侧栏" }));
    const rail = within(
      await screen.findByRole("complementary", { name: "上下文侧栏" }),
    );
    await user.click(rail.getByRole("button", { name: /文件/ }));
    // 文件行可点（以前是只读 div）
    const row = await rail.findByTitle(/报告\.md/);
    expect(row.tagName).toBe("BUTTON");
    await user.click(row);
    // 内容在侧栏里就地渲染，不跳出去——默认看的是本会话的改动
    expect(await rail.findByText("+新标题")).toBeInTheDocument();
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_file_changes", {
      sessionId: "s1",
      path: "/tmp/报告.md",
    });
    // 全文仍是有的路：切过去就地渲染，再能退回改动视图
    await user.click(rail.getByRole("button", { name: "看全文" }));
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_preview_text", {
      path: "/tmp/报告.md",
    });
    expect(await rail.findByLabelText("文件内容")).toBeInTheDocument();
    expect(rail.getByText("标题")).toBeInTheDocument();
    // 能退回列表
    await user.click(rail.getByText(/返回文件列表/));
    await waitFor(() =>
      expect(rail.queryByLabelText("文件内容")).toBeNull(),
    );
  });

  it("授权模式可点选：写入走窄命令 bot_set_perm_mode，落盘失败回滚", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_set_perm_mode") throw { code: "IO", message: "只读磁盘" };
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 默认档是 ask（bot_get_config 没给 permMode 时按安全默认归一）
    const pill = await waitFor(() =>
      screen.getByRole("button", { name: /弹授权/ }),
    );
    // 以前这里是个只读 span，点不动——正是要改的地方
    expect(pill.tagName).toBe("BUTTON");
    await user.click(pill);
    const opt = await screen.findByRole("option", { name: /全放行/ });
    await user.click(opt);
    // 写盘走窄命令（只动 perm_mode），不是整份 bot_set_config 回写
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_perm_mode", {
      mode: "yolo",
    });
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_set_config"),
    ).toBe(false);
    // 落盘失败 → 回滚到 ask 并弹错，不留「界面显示已放行」的假绿
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /弹授权/ })).toBeInTheDocument(),
    );
  });

  it("授权模式成功落盘后 pill 切换到新档位（yolo 走警示色）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return { ...((await chatDefaultInvoke("bot_get_config")) as object), permMode: "ask" };
      if (cmd === "bot_set_perm_mode") return chatDefaultInvoke("bot_get_config");
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const pill = await screen.findByRole("button", { name: /弹授权/ });
    await user.click(pill);
    await user.click(await screen.findByRole("option", { name: /自动/ }));
    const next = await screen.findByRole("button", { name: /自动/ });
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_perm_mode", {
      mode: "auto",
    });
    // 下拉选完自动收起（选完还开着会挡住下一次点发送）
    await waitFor(() =>
      expect(screen.queryByRole("option", { name: /全放行/ })).toBeNull(),
    );
    expect(next).toBeInTheDocument();
  });

  it("点 🧠 弹出当前协议模型列表，再点收起", async () => {
    const user = userEvent.setup();
    // 前面用例换过的 mockImplementation 会跨用例残留（beforeEach 只 mockClear），
    // 先恢复默认实现拿到 bot_get_config / bot_set_active_model 的 mock
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 等 bot_get_config reload 完成（chip 标签与 setModels 同一个 .then 刷新）；
    // chip 与下拉项同名，一律在 chip 按钮内查询（findByText 作用于按钮本身）
    const chip = screen.getByTitle("切换模型");
    await waitFor(() => {
      expect(chip).toHaveTextContent("MiniMax-M3");
    });
    await user.click(chip);
    expect(await screen.findByText("DeepSeek-V3")).toBeInTheDocument();
    // chip 与下拉项同名：chip 用 toHaveTextContent，下拉项按 class 限定
    expect(chip).toHaveTextContent("MiniMax-M3");
    // 再点收起
    await user.click(chip);
    await waitFor(() => {
      expect(screen.queryByText("DeepSeek-V3")).toBeNull();
    });
  });

  it("模型下拉可用性过滤：带厂商但未通过连接测试的模型不显示，已验证的显示", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.deepseek.com",
          model: "deepseek-chat",
          apiProvider: "openai",
          modelsByProvider: {
            openai: [
              {
                id: "m1",
                label: "DeepSeek Chat",
                baseUrl: "https://api.deepseek.com",
                model: "deepseek-chat",
                vendor: "DeepSeek",
              },
              {
                id: "m2",
                label: "Kimi K3",
                baseUrl: "https://api.moonshot.cn/v1",
                model: "kimi-k3",
                vendor: "Kimi",
              },
              {
                id: "m3",
                label: "自建直连",
                baseUrl: "https://llm.example.com/v1",
                model: "my-model",
              },
            ],
            anthropic: [],
          },
          activeModelId: { openai: "m1", anthropic: null },
          // 只有 Kimi 通过了连接测试
          verifiedVendors: ["Kimi"],
        };
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const chip = screen.getByTitle("切换模型");
    await user.click(chip);
    // Kimi 已验证 → 显示；DeepSeek 未验证 → 不显示；无 vendor 的自建条目不受影响
    expect(await screen.findByText("Kimi K3")).toBeInTheDocument();
    expect(screen.queryByText("DeepSeek Chat")).toBeNull();
    expect(screen.getByText("自建直连")).toBeInTheDocument();
  });

  // ── SWITCH-1：busy 期实时切换对话，回复不串台 ──

  it("busy 中切换/新建/删除（拦回复中会话），回复仍落原会话", async () => {
    const user = userEvent.setup();
    // 捕获事件监听器（手动触发流式增量用）
    const evListeners: Record<string, ((e: { payload: unknown }) => void)[]> =
      {};
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (ev: string, cb: (e: { payload: unknown }) => void) => {
        (evListeners[ev] ??= []).push(cb);
        return () => {};
      },
    );
    const historyLoads: unknown[] = [];
    const saves: Array<Record<string, unknown> | undefined> = [];
    let releaseChat!: (v: { text: string; taskRefs: [] }) => void;
    const chatGate = new Promise<{ text: string; taskRefs: [] }>((r) => {
      releaseChat = r;
    });
    const prevInvokeImpl = mocks.invokeMock.getMockImplementation();
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        switch (cmd) {
          case "bot_sessions_load":
            return [
              { id: "s1", title: "默认会话" },
              { id: "s2", title: "另一个" },
            ];
          case "bot_chat":
            return chatGate;
          case "bot_history_load":
            historyLoads.push(args?.sessionId);
            return [];
          case "bot_history_save":
            saves.push(args);
            return null;
          case "bot_session_create":
            return { id: "s-new", title: "新对话" };
          default:
            return chatDefaultInvoke(cmd);
        }
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      // 发送 → bot_chat 挂起 → busy
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "你好");
      await user.keyboard("{Enter}");
      await waitFor(() => {
        expect(mocks.invokeMock).toHaveBeenCalledWith(
          "bot_chat",
          expect.objectContaining({ sessionId: "s1" }),
        );
      });
      // busy 中切换到 s2 → 允许（回归钉 1：不再被 busy 拦截）
      await user.click(railSession("另一个"));
      await waitFor(() => {
        expect(historyLoads).toContain("s2");
      });
      // 向 s1（后台回复中）发流式增量 → 不得污染当前 s2 视图
      await act(async () => {
        for (const cb of evListeners["bot-chat-delta"] ?? []) {
          cb({ payload: { sessionId: "s1", text: "串台密文" } });
        }
      });
      expect(screen.queryByText(/串台密文/)).toBeNull();
      // busy 中删除正在回复的 s1 → 拦截（不弹 confirm、不调 delete）
      mocks.askMock.mockReset();
      const s1Row = railSession("默认会话").closest("div");
      // 删除键悬停才显形（列表不留白抖动），先 hover 再点
      await user.hover(s1Row!);
      await user.click(within(s1Row!).getByTitle("删除此对话"));
      expect(
        mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_session_delete"),
      ).toBe(false);
      expect(mocks.askMock).not.toHaveBeenCalled();
      // 删除守卫返回后侧栏仍在：busy 中新建对话 → 允许（回归钉 2）
      await user.click(screen.getByText("新建对话"));
      await waitFor(() => {
        expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_create", {
          title: null,
        });
      });
      // 切回 s1（回复仍在进行）→ 允许并补占位气泡（s1 的 history_load 比挂载时 +1）
      const s1LoadsBefore = historyLoads.filter((x) => x === "s1").length;
      await user.click(railSession("默认会话"));
      expect(historyLoads.filter((x) => x === "s1").length).toBe(
        s1LoadsBefore + 1,
      );
      // 收尾：回复落回原会话 s1（不串台核心钉 3），绝不写 s2
      await act(async () => {
        releaseChat({ text: "回复A", taskRefs: [] });
      });
      await waitFor(() => {
        expect(saves.length).toBeGreaterThan(0);
      });
      for (const sv of saves) {
        expect(sv?.sessionId).toBe("s1");
      }
      const lastSave = saves[saves.length - 1] as {
        sessionId: string;
        messages: { role: string; content: string }[];
      };
      expect(lastSave.messages[lastSave.messages.length - 1].content).toBe(
        "回复A",
      );
    } finally {
      mocks.invokeMock.mockImplementation(prevInvokeImpl!);
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });
});
