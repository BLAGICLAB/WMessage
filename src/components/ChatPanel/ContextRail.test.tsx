import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ContextRail, type SubagentRun } from "./ContextRail";
import type { Task } from "../../types";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openTargetMock: vi.fn(),
  listenMock: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listenMock,
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
      sessionId={null}
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

  // 复现：侧栏此前拿「点击时的 task 快照」当合并基线——对话框挂起期间
  // 另一条路径（卡片 / 详情面板）绑进来的文件会被整列覆盖掉。
  it("绑定对话框挂起期间 tasks 被并发改动：合并基线取最新，两批文件都不丢", async () => {
    const user = userEvent.setup();
    const onUpdateTask = vi.fn();
    let releasePick!: (v: string[]) => void;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "pick_files_dialog")
        return new Promise<string[]>((resolve) => {
          releasePick = resolve;
        });
      if (cmd === "bind_files")
        return [{ path: "/Users/t/Desktop/新.txt", isDir: false }];
      return null;
    });
    const { rerender } = render(
      <ContextRail
        tasks={[task]}
        onUpdateTask={onUpdateTask}
        onRefTask={vi.fn()}
        refIds={[]}
        sessionId={null}
        subagents={[]}
        onOpenSubagent={vi.fn()}
        onCancelSubagent={vi.fn()}
        tab="tasks"
        onTabChange={vi.fn()}
      />,
    );
    await user.click(screen.getByLabelText("绑定文件到 写周报"));
    // 对话框挂起期间：另一条路径给这张卡绑了 并发.txt
    rerender(
      <ContextRail
        tasks={[
          {
            ...task,
            files: [{ path: "/Users/t/Desktop/并发.txt", isDir: false }],
          },
        ]}
        onUpdateTask={onUpdateTask}
        onRefTask={vi.fn()}
        refIds={[]}
        sessionId={null}
        subagents={[]}
        onOpenSubagent={vi.fn()}
        onCancelSubagent={vi.fn()}
        tab="tasks"
        onTabChange={vi.fn()}
      />,
    );
    await act(async () => {
      releasePick(["/Users/t/Desktop/新.txt"]);
    });
    // 陈旧基线（无绑定）会把 并发.txt 整列覆盖掉
    expect(onUpdateTask).toHaveBeenCalledWith("t1", {
      files: [
        { path: "/Users/t/Desktop/并发.txt", isDir: false },
        { path: "/Users/t/Desktop/新.txt", isDir: false },
      ],
    });
  });
});

// 文件工作面：清单按后端口径取（本会话累计 ±），点开默认看「改动」而不是全文。
// 口径由后端算（累计 / 逐次 / 无行级 diff 三档），前端不自己累加。
describe("ContextRail 文件改动工作面", () => {
  const PATH = "/Users/x/调研.md";
  const item = {
    path: PATH,
    kind: "modify",
    added: 7,
    deleted: 2,
    changes: 2,
    mode: "session",
    drifted: false,
    note: null,
  };
  const detail = {
    path: PATH,
    kind: "modify",
    mode: "session",
    added: 7,
    deleted: 2,
    changes: 2,
    truncated: false,
    drifted: false,
    diff: `--- ${PATH}\n+++ ${PATH}\n@@ -1,2 +1,2 @@\n-旧标题\n+新标题`,
    perChange: [],
    note: null,
  };
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
  /** 后端口径模拟：带 path 的是详情请求，不带的是清单请求 */
  function mockSessionFiles(
    items: unknown[] = [item],
    d: unknown = detail,
    previewPath = PATH,
  ) {
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: { path?: string }) => {
        if (cmd === "bot_session_file_changes") {
          return args?.path
            ? { items: [], detail: d }
            : { items, detail: null };
        }
        if (cmd === "bot_preview_text") return previewOf(previewPath);
        return null;
      },
    );
  }

  it("清单按后端口径取：显示累计 ±，不是前端累加的数字", async () => {
    mockSessionFiles();
    renderRail({ tab: "files", sessionId: "s1" });
    expect(await screen.findByText("+7")).toBeInTheDocument();
    expect(screen.getByText("−2")).toBeInTheDocument();
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_file_changes", {
      sessionId: "s1",
    });
  });

  it("点文件行：切到改动视图并报给 onPreviewReq，不调起系统默认程序", async () => {
    const user = userEvent.setup();
    const onPreviewReq = vi.fn();
    mockSessionFiles();
    renderRail({ tab: "files", sessionId: "s1", onPreviewReq });
    await user.click(await screen.findByTitle(/点开看改动/));
    expect(onPreviewReq).toHaveBeenCalledWith({ path: PATH, mode: "diff" });
    expect(mocks.openTargetMock).not.toHaveBeenCalled();
  });

  it("改过的文件默认看累计 diff：拉详情、渲染改动，不读全文", async () => {
    mockSessionFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      previewReq: { path: PATH, mode: "diff" },
    });
    expect(await screen.findByText("+新标题")).toBeInTheDocument();
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_file_changes", {
      sessionId: "s1",
      path: PATH,
    });
    // 默认不读全文——全文是另一个视图，用户点「看全文」才读
    expect(mocks.invokeMock).not.toHaveBeenCalledWith(
      "bot_preview_text",
      expect.anything(),
    );
  });

  it("「看全文」切到全文视图并读快照；再点「看改动」能回来", async () => {
    const user = userEvent.setup();
    mockSessionFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      previewReq: { path: PATH, mode: "diff" },
    });
    await user.click(await screen.findByRole("button", { name: "看全文" }));
    expect(await screen.findByLabelText("文件内容")).toHaveTextContent(
      "上海博士后政策",
    );
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_preview_text", {
      path: PATH,
    });
    await user.click(screen.getByRole("button", { name: "看改动" }));
    expect(await screen.findByText("+新标题")).toBeInTheDocument();
  });

  it("没改过的路径（正文链接打开的）→ 直接看全文，不发详情请求", async () => {
    const other = "/Users/x/别的.md";
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_session_file_changes") return { items: [item], detail: null };
      if (cmd === "bot_preview_text") return previewOf(other);
      return null;
    });
    renderRail({
      tab: "files",
      sessionId: "s1",
      previewReq: { path: other, mode: "content" },
      onPreviewReq: vi.fn(),
    });
    expect(await screen.findByLabelText("文件内容")).toHaveTextContent(
      "上海博士后政策",
    );
    expect(mocks.invokeMock).not.toHaveBeenCalledWith(
      "bot_session_file_changes",
      expect.objectContaining({ path: other }),
    );
  });

  it("降级档（给不出累计）必须说清原因——口径变了却不说，用户会误读", async () => {
    mockSessionFiles([{ ...item, mode: "perChange", note: "起始快照缺失" }], {
      ...detail,
      mode: "perChange",
      diff: null,
      perChange: [
        {
          id: 1,
          kind: "modify",
          added: 3,
          deleted: 1,
          diff: `--- ${PATH}\n+++ ${PATH}\n@@ -1 +1 @@\n-旧\n+新`,
          truncated: false,
        },
      ],
      note: "本会话第一次修改前的快照缺失；下面是本会话的逐次改动",
    });
    renderRail({
      tab: "files",
      sessionId: "s1",
      previewReq: { path: PATH, mode: "diff" },
    });
    expect(
      await screen.findByText(/快照缺失/),
    ).toBeInTheDocument();
    expect(screen.getByText("第 1 次改动 ·")).toBeInTheDocument();
  });

  it("返回文件列表：回到列表而不是空白", async () => {
    const user = userEvent.setup();
    const onPreviewReq = vi.fn();
    mockSessionFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      previewReq: { path: PATH, mode: "diff" },
      onPreviewReq,
    });
    await user.click(await screen.findByText("返回文件列表"));
    expect(onPreviewReq).toHaveBeenCalledWith(null);
  });
});

// 右侧栏文件页的「改动 / 交付物」双子页：两类东西不混在一张清单里。
// 交付物 = 这个会话的产物（正文自报 ∪ 会话写盘），改动 = 这个会话动过的
// 文件（带 ±行 / diff）。
describe("ContextRail 文件页双子页", () => {
  const CHANGED = "/Users/x/生成脚本.py";
  const PPTX = "/Users/x/宣贯.pptx";
  const DOC = "/Users/x/设计说明.md";

  function mockFiles(
    opts: {
      items?: unknown[];
      missing?: string[];
      previewPath?: string;
    } = {},
  ) {
    const items = opts.items ?? [
      {
        path: CHANGED,
        kind: "modify",
        added: 462,
        deleted: 0,
        changes: 1,
        mode: "session",
        drifted: false,
        note: null,
      },
    ];
    const missing = new Set(opts.missing ?? []);
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: { paths?: string[]; path?: string }) => {
        if (cmd === "bot_session_file_changes") {
          return args?.path ? { items: [], detail: null } : { items, detail: null };
        }
        if (cmd === "bot_probe_paths") {
          return (args?.paths ?? []).map((p) => ({
            path: p,
            exists: !missing.has(p),
            isDir: false,
            size: 1,
          }));
        }
        if (cmd === "bot_preview_text") {
          return {
            kind: "text",
            path: opts.previewPath ?? DOC,
            content: "# 设计说明\n正文一段",
            truncated: false,
            totalBytes: 20,
            lines: 2,
          };
        }
        if (cmd === "bot_open_gen_dir") return "/Users/x/AI_Gen_Files";
        return null;
      },
    );
  }

  const sessionMessages = [
    { role: "user", content: "参考 /Users/x/输入稿.md" },
    { role: "assistant", content: `已生成 ${PPTX} 与 ${DOC}` },
  ];

  it("文件页顶部有「打开产物文件夹」：点击调后端并回显解析后的路径", async () => {
    const user = userEvent.setup();
    mockFiles();
    renderRail({ tab: "files", sessionId: "s1" });
    await user.click(
      screen.getByRole("button", { name: /打开产物文件夹/ }),
    );
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_open_gen_dir");
    });
    expect(
      screen.getByTitle("打开产物文件夹：/Users/x/AI_Gen_Files"),
    ).toBeInTheDocument();
  });

  it("子页可切：点「交付物」报给 onFileSubTabChange", async () => {
    const user = userEvent.setup();
    const onFileSubTabChange = vi.fn();
    mockFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "changes",
      onFileSubTabChange,
    });
    await user.click(screen.getByRole("tab", { name: /交付物/ }));
    expect(onFileSubTabChange).toHaveBeenCalledWith("deliverables");
  });

  it("改动页是改动清单（现有清单一行不改）", async () => {
    mockFiles();
    renderRail({ tab: "files", sessionId: "s1", fileSubTab: "changes" });
    expect(await screen.findByText("生成脚本.py")).toBeInTheDocument();
    expect(screen.getByText("+462")).toBeInTheDocument();
  });

  it("交付物页是会话级清单：正文点名的挂上，用户消息里的引用不算", async () => {
    mockFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
    });
    expect(await screen.findByText("宣贯.pptx")).toBeInTheDocument();
    expect(screen.getByText("设计说明.md")).toBeInTheDocument();
    // 用户消息里的路径是「让它读那个文件」，不是交付
    expect(screen.queryByText("输入稿.md")).toBeNull();
    // 写盘通道记的脚本形态不进交付物位
    expect(screen.queryByText("生成脚本.py")).toBeNull();
  });

  it("磁盘上不存在的路径不进交付物清单", async () => {
    mockFiles({ missing: [DOC] });
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
    });
    expect(await screen.findByText("宣贯.pptx")).toBeInTheDocument();
    expect(screen.queryByText("设计说明.md")).toBeNull();
  });

  it("点交付物走内容预览（mode=content），不看差异", async () => {
    const user = userEvent.setup();
    const onPreviewReq = vi.fn();
    mockFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
      onPreviewReq,
    });
    await user.click(
      await screen.findByTitle(`${PPTX}（回复中点名）· 点开预览`),
    );
    expect(onPreviewReq).toHaveBeenCalledWith({ path: PPTX, mode: "content" });
  });
});

// 文件页重设计的视觉契约：类型铭牌是唯一的身份设备，区头不再复读页签，
// 空态给去向不留死文本。
describe("ContextRail 文件页视觉契约", () => {
  const PPTX = "/Users/x/宣贯.pptx";
  const DOC = "/Users/x/设计说明.md";

  function mockFiles(opts: { missing?: string[] } = {}) {
    const missing = new Set(opts.missing ?? []);
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: { paths?: string[]; path?: string }) => {
        if (cmd === "bot_session_file_changes") {
          return args?.path
            ? { items: [], detail: null }
            : { items: [], detail: null };
        }
        if (cmd === "bot_probe_paths") {
          return (args?.paths ?? []).map((p) => ({
            path: p,
            exists: !missing.has(p),
            isDir: false,
            size: 1,
          }));
        }
        return null;
      },
    );
  }

  const sessionMessages = [
    { role: "assistant", content: `已生成 ${PPTX} 与 ${DOC}` },
  ];

  it("交付物行带类型铭牌：扩展名大写缩进 24px 方块", async () => {
    mockFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
    });
    // 清单有 200ms 防抖 + 探测异步，用 findBy 等渲染
    expect(await screen.findByText("PPTX")).toBeInTheDocument();
    expect(screen.getByText("MD")).toBeInTheDocument();
  });

  it("区头不再复读页签：名字与计数只在页签上一份", () => {
    mockFiles();
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
    });
    expect(screen.queryByText("本会话交付物")).toBeNull();
  });

  it("空态是邀请不是死文本：主行 + 去向一句", async () => {
    mockFiles({ missing: [PPTX, DOC] });
    renderRail({
      tab: "files",
      sessionId: "s1",
      fileSubTab: "deliverables",
      sessionMessages,
    });
    expect(
      await screen.findByText("本会话还没有交付物"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("机器人产出的文件会汇总在这里，点开即预览"),
    ).toBeInTheDocument();
  });
});

// 右侧栏列表行瘦身为「名称 + 用时 + 停止键」：状态标签、轮数/调用数进度
// 与逐步动作流都进详情页——列表塞满数字就成了仪表盘，看过程点行进去。
describe("ContextRail 子 Agent 列表行", () => {
  const RUNNING_ROW: SubagentRun = {
    id: "sa_1",
    title: "读 SPEC 出素材稿",
    status: "running",
    running: true,
    roundsUsed: 33,
    roundsBudget: 50,
    toolCallsUsed: 98,
    toolCallsBudget: 100,
    activity: [
      { toolCallId: "t1", tool: "grep_files", ok: true, ms: 4, preview: "P: 并发", at: 1 },
    ],
  };

  it("行上只有名称、用时和停止键；进度读数与动作流不进列表", () => {
    // startedAt 取当下：describe 收集到用例执行间隔不定，固定值会让耗时在 0s/1s 间漂
    renderRail({
      tab: "agents",
      subagents: [{ ...RUNNING_ROW, startedAt: Date.now() }],
    });
    expect(screen.getByText("读 SPEC 出素材稿")).toBeInTheDocument();
    expect(screen.getByText("0s")).toBeInTheDocument();
    expect(screen.getByLabelText("取消子 agent：读 SPEC 出素材稿")).toBeInTheDocument();
    expect(screen.queryByText("33/50 轮")).toBeNull();
    expect(screen.queryByText("98/100 次")).toBeNull();
    expect(screen.queryByText("执行中")).toBeNull();
    expect(screen.queryByText("grep_files")).toBeNull();
  });

  it("六态行都能渲染（图标色查表不炸）", () => {
    const STATES: SubagentRun["status"][] = [
      "queued",
      "running",
      "succeeded",
      "failed",
      "cancelled",
      "budget_exceeded",
    ];
    renderRail({
      tab: "agents",
      subagents: STATES.map((status, i) => ({
        id: `sa_${i}`,
        title: `子任务${i}`,
        status,
        running: status === "running" || status === "queued",
      })),
    });
    for (let i = 0; i < STATES.length; i++) {
      expect(screen.getByText(`子任务${i}`)).toBeInTheDocument();
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
