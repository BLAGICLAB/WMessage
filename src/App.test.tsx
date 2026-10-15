import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import { stubScrollNoop } from "./test/scrollNoop";
import type { Task } from "./types";

// 聊天区常挂载在主窗口，自动滚动依赖 jsdom 未实现的 scrollTo/scrollIntoView
stubScrollNoop();

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const openMock = vi.fn();
  const saveMock = vi.fn();
  const askMock = vi.fn();
  return { invokeMock, listenMock, openMock, saveMock, askMock };
});

// 夹具任务：多数用例拿它当「首屏已加载」的信号。用它而不是演示数据，是因为
// 演示数据曾经既是 UI 的假卡来源、又是测试的同步信号——耦合一处，删假卡就断一片。
const FIXTURE_TITLE = "夹具任务·甲";
const FIXTURE: Task = {
  id: "fx1",
  title: FIXTURE_TITLE,
  column: "todo",
  order: 0,
  updatedAt: 1,
};

const defaultInvokeImpl = async (cmd: string) => {
  if (cmd === "db_load") return [FIXTURE]; // 空库由「不注入假卡」用例专门覆盖
  if (cmd === "db_upsert") return null;
  if (cmd === "db_delete") return null;
  if (cmd === "workspace_load") return []; // 工作区空列表
  if (cmd === "workspace_upsert") return null;
  if (cmd === "workspace_delete") return null;
  if (cmd === "tasks_export") return 3;
  return null;
};
mocks.invokeMock.mockImplementation(defaultInvokeImpl);
mocks.listenMock.mockImplementation(async () => () => {});
mocks.openMock.mockImplementation(async () => null);
mocks.saveMock.mockImplementation(async () => null);
mocks.askMock.mockResolvedValue(true);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: mocks.listenMock,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
  save: mocks.saveMock,
  ask: mocks.askMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
  openUrl: vi.fn(async () => {}),
}));

// alert：vitest 用 spy 替身，避免 jsdom 弹原生弹框卡住测试
const alertMock = vi.fn();
beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.invokeMock.mockImplementation(defaultInvokeImpl); // 恢复默认实现（个别用例会覆盖）
  mocks.listenMock.mockClear();
  mocks.openMock.mockClear();
  mocks.saveMock.mockClear();
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
  alertMock.mockClear();
  window.alert = alertMock;
});

describe("App", () => {
  it("初始渲染：首页视图 + 库内任务上任务池", async () => {
    render(<App />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("db_load");
    });
    // 启动默认定位首页
    expect(screen.getByTestId("home-page")).toBeInTheDocument();
    // 任务页 = 周时间网格：列头 + 时刻刻度 + 任务池（批 2 只读渲染）——点导航切换过去
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    expect(screen.getByText("周一")).toBeInTheDocument();
    expect(screen.getByText("08:00")).toBeInTheDocument();
    expect(screen.getByLabelText("任务池")).toBeInTheDocument();
    // 库内任务上池（UI 只渲染 db_load 给的数据，没有任何自带数据源）
    expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    // 顶栏：任务 / 归档 / 工作区 / 回收站
    expect(screen.getByText("任务")).toBeInTheDocument();
    expect(screen.getByText("归档")).toBeInTheDocument();
    expect(screen.getByText("工作区")).toBeInTheDocument();
    expect(screen.getByText("回收站")).toBeInTheDocument();
  });

  // 回归锁：空库不得注入任何数据。
  // 事故复现——数据目录被解析到另一个库（便携/隔离目录漂移）时 db_load 同样返回
  // []，曾经的 SEED 分支会往那个库里写 3 张演示卡，用户看到「只剩 3 个任务」，
  // 且真库里的数据一条没少却无从解释。空 = 空，写入只允许来自用户操作。
  it("空库（db_load = []）：不写库、不注入任何任务卡", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return [];
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("db_load");
    });
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    expect(screen.getByLabelText("任务池")).toBeInTheDocument();
    expect(screen.queryByText(FIXTURE_TITLE)).not.toBeInTheDocument();
    // 关键断言：一次 db_upsert 都不该发生（空库没有可写的东西）
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_upsert"),
    ).toHaveLength(0);
  });

  it("点击「归档」按钮：视图切到 ArchivePage（搜索框出现 / 列头消失）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: "归档" }));
    // 归档页：暂无归档内容（seed 任务都没 archived 标记）
    expect(await screen.findByText("暂无归档内容")).toBeInTheDocument();
    // 首页骨架不再显示
    expect(screen.queryByTestId("home-page")).not.toBeInTheDocument();
  });

  it("点击「工作区」：视图切到 WorkspacePage（显示「+ 新建工作区」）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    await user.click(screen.getByText("工作区"));
    expect(await screen.findByText("+ 新建工作区")).toBeInTheDocument();
    // 任务页骨架消失
    expect(screen.queryByLabelText("任务池")).not.toBeInTheDocument();
  });

  it("mutate 流程：点击「新建任务」→ db_upsert 收到新任务 + 标题进入编辑态", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    const callsBefore = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "db_upsert",
    ).length;
    // 新建任务按钮迁入左侧导航栏（文案从「+ 新建任务」改为图标 + 「新建任务」）
    await user.click(screen.getByText("新建任务"));
    // 不切视图：浮窗覆盖当前页（首页骨架仍在场），从左滑出新建编辑浮窗
    expect(screen.getByTestId("home-page")).toBeInTheDocument();
    // 新建后详情浮窗打开，标题自动聚焦进入编辑态
    const input = await screen.findByDisplayValue("新任务");
    expect(input).toBeInTheDocument();
    // mutate → upsertTasks → db_upsert 被多调用至少一次
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert",
      );
      expect(calls.length).toBeGreaterThan(callsBefore);
    });
  });

  it("点击「回收站」：视图切到 TrashPage（默认显示「暂无回收站内容」）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    await user.click(screen.getByText("回收站"));
    expect(
      await screen.findByText(/暂无回收站内容|回收站是空的/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("任务池")).not.toBeInTheDocument();
  });

  it("db_load 读失败：不走迁移分支、不写库、弹告警（error ≠ empty）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") throw new Error("database is locked");
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    // 明确告警（批 4 起走应用内错误弹窗，ErrorDialogHost 随 App 挂载）
    const errDialog = await screen.findByRole("alertdialog");
    expect(errDialog.textContent).toContain("database is locked");
    // 不触发迁移写入，也不删除任何行
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_upsert"),
    ).toHaveLength(0);
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_delete"),
    ).toHaveLength(0);
    // 启动默认在首页：切到任务页再验证空池
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    // 任务页仍渲染（内存空数组）：网格骨架 + 空池，库内任务不出现
    expect(screen.getByLabelText("任务池")).toBeInTheDocument();
    expect(
      screen.queryByText(FIXTURE_TITLE),
    ).not.toBeInTheDocument();
  });

  // mutate 落盘失败必须抛错、tasksRef/state 不得先行更新——
  // 修复前 tasksRef.current = next 在 await 落盘之前，失败时 UI 已更新但磁盘没动
  it("SYNC-1：tasks-updated 合并与 UI mutate 共串行链（merge await 期间 UI 写排队，写冲突回归钉）", async () => {
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    // 排掉初始装载尾巴（规则回写等），避免占用 db_upsert 门闩
    await act(async () => {});
    const upsertCalls = () =>
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_upsert").length;
    const callsBefore = upsertCalls();
    // 捕获 tasks-updated 监听器回调
    const lu = mocks.listenMock.mock.calls.find(
      ([ev]) => ev === "tasks-updated",
    );
    expect(lu).toBeTruthy();
    const onTasksUpdated = lu![1] as (e: { payload: unknown }) => void;
    // 第一次 db_upsert（事件合并回写）挂起在门闩上
    let release!: () => void;
    const gate = new Promise<void>((r) => {
      release = r;
    });
    mocks.invokeMock.mockImplementationOnce(async (cmd: string) => {
      if (cmd === "db_upsert") {
        await gate;
        return null;
      }
      return defaultInvokeImpl(cmd);
    });
    // 无 source 的协议外上报（→ 主窗回写路径）
    await act(async () => {
      onTasksUpdated({
        payload: {
          upserts: [
            {
              id: "evt-1",
              title: "事件合并任务",
              column: "todo",
              order: 99,
              updatedAt: 1234000,
              expectedUpdatedAt: 1234000,
            },
          ],
          deletes: [],
        },
      });
    });
    await waitFor(() => {
      expect(upsertCalls()).toBe(callsBefore + 1);
    });
    // 核心断言：merge 的落盘还挂在门闩上时，UI 写（新建任务）不得出队——
    // 修复前两条独立链会让它立即落盘（基线跨链失效 → 写冲突弹窗的根因）
    await userEvent.setup().click(screen.getByText("新建任务"));
    expect(upsertCalls()).toBe(callsBefore + 1);
    // 放行 merge → UI 写才落盘
    release();
    await waitFor(() => {
      expect(upsertCalls()).toBe(callsBefore + 2);
    });
  });

  it("mutate 落盘失败：抛错 + UI 不更新（tasksRef 未先行赋值）", async () => {
    const user = userEvent.setup();
    const dbErr = {
      code: "DB_ERROR",
      message: "disk full",
      recoverable: false,
    };
    let failUpsert = false;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_upsert" && failUpsert) throw dbErr;
      return defaultInvokeImpl(cmd);
    });
    // mutate 抛错 → fire-and-forget 入口（mutateFire）catch 后 console 留痕
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(<App />);
      await waitFor(() => {
        expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
      });
      failUpsert = true; // 初始化完成后，后续写入全部失败
      alertMock.mockClear();
      // emit mock 跨用例共享（beforeEach 不清），先清零再断言「失败不广播」
      const { emit } = await import("@tauri-apps/api/event");
      (emit as ReturnType<typeof vi.fn>).mockClear();
      await user.click(screen.getByText("新建任务"));
      // storage 层错误弹窗已弹（upsertTasks → handleCommandError → ErrorDialogHost）
      await waitFor(() => expect(screen.getByRole("alertdialog")).toBeTruthy());
      // mutate 抛错（错误从 upsertTasks 一路抛到 call site 的 catch）
      await waitFor(() =>
        expect(errSpy).toHaveBeenCalledWith("[mutate] persist failed", dbErr),
      );
      // tasksRef 未先行赋值 → 新任务不进 UI（不会出现标题编辑 input）
      expect(screen.queryByDisplayValue("新任务")).not.toBeInTheDocument();
      // 失败不广播（不得让别处读到未落盘的中间态）
      expect(emit).not.toHaveBeenCalledWith("tasks-changed");
    } finally {
      errSpy.mockRestore();
    }
  });

  // 软删除必须清调度字段——否则任务躺在回收站里 schedule
  // 仍到点触发（deletedAt 不挡调度读取）。
  // 入口：任务页池项 → 详情面板删除键（批 4 起替代旧看板卡删除键）
  it("软删除进回收站：task_patch 只带 deletedAt", async () => {
    const user = userEvent.setup();
    const withSched: Task[] = [
      {
        id: "s1",
        title: "定时任务",
        column: "todo",
        order: 0,
        updatedAt: 1,
      },
    ];
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return withSched;
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    // 启动默认在首页：切到任务页再操作池块
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    // 池块单击纯拖拽（防误触）→ 双击池项开详情面板
    await user.dblClick(await screen.findByText("定时任务"));
    await user.click(await screen.findByTitle("删除任务"));
    // TP-2：软删走 task_patch 定向补丁
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "task_patch",
      );
      expect(calls.length).toBeGreaterThan(0);
      const args = calls[0][1] as {
        id: string;
        patch: Record<string, unknown>;
      };
      expect(args.id).toBe("s1");
      expect(args.patch.deletedAt).toBeTruthy();
    });
  });

  // 子任务卡：删除有子卡的母卡时先询问是否连带（okLabel=一并删除）
  const motherWithChildren: Task[] = [
    { id: "m1", title: "母卡", column: "todo", order: 0, updatedAt: 1 },
    // 子卡放完成列：不上任务池（已完成区折叠），池内只有母卡一个双击入口
    {
      id: "c1",
      title: "子卡一",
      column: "done",
      order: 1,
      updatedAt: 1,
      parentId: "m1",
    },
    {
      id: "c2",
      title: "子卡二",
      column: "done",
      order: 2,
      updatedAt: 1,
      parentId: "m1",
    },
  ];
  const patchedIds = () =>
    mocks.invokeMock.mock.calls
      .filter((c) => c[0] === "task_patch")
      .map((c) => (c[1] as { id: string }).id);

  it("删除母卡询问子卡：选「一并删除」→ 母卡与全部子卡都收 task_patch", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return motherWithChildren;
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    await user.dblClick(await screen.findByText("母卡"));
    await user.click(await screen.findByTitle("删除任务"));
    expect(mocks.askMock).toHaveBeenCalledTimes(1);
    expect(mocks.askMock).toHaveBeenCalledWith(
      expect.stringContaining("有 2 张子任务卡"),
      expect.objectContaining({ okLabel: "一并删除子卡", cancelLabel: "仅删母卡" }),
    );
    await waitFor(() => {
      expect(patchedIds().sort()).toEqual(["c1", "c2", "m1"]);
    });
  });

  it("删除母卡询问子卡：选「仅删母卡」→ 子卡不动", async () => {
    const user = userEvent.setup();
    mocks.askMock.mockResolvedValue(false);
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return motherWithChildren;
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    await user.dblClick(await screen.findByText("母卡"));
    await user.click(await screen.findByTitle("删除任务"));
    await waitFor(() => {
      expect(patchedIds()).toEqual(["m1"]);
    });
  });

  it("子任务行一键生成任务卡：新卡带 parentId 进待办，原行回写 taskId", async () => {
    const user = userEvent.setup();
    const mother: Task = {
      id: "m1",
      title: "母卡",
      column: "todo",
      order: 0,
      updatedAt: 1,
      subtasks: [{ id: "s1", text: "待生成", done: false }],
    };
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return [mother];
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    await user.dblClick(await screen.findByText("母卡"));
    await user.click(await screen.findByTitle("生成为任务卡"));
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert",
      );
      expect(calls.length).toBeGreaterThan(0);
      const { tasks } = calls[calls.length - 1][1] as { tasks: Task[] };
      const child = tasks.find((t) => t.parentId === "m1");
      expect(child?.column).toBe("todo");
      expect(child?.title).toBe("待生成");
      const motherRow = tasks.find((t) => t.id === "m1");
      expect(motherRow?.subtasks?.[0]?.taskId).toBe(child?.id);
    });
  });

  it("子任务行 ×：删行 + 子卡解除关联保留（parentId 清空，卡不删）", async () => {
    const user = userEvent.setup();
    const mother: Task = {
      id: "m1",
      title: "母卡",
      column: "todo",
      order: 0,
      updatedAt: 1,
      subtasks: [{ id: "s1", text: "已生成", done: false, taskId: "c1" }],
    };
    const child: Task = {
      id: "c1",
      title: "已生成",
      column: "todo",
      order: 1,
      updatedAt: 1,
      parentId: "m1",
    };
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return [mother, child];
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    await user.dblClick(await screen.findByText("母卡"));
    await user.click(await screen.findByTitle("删除子任务"));
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert",
      );
      expect(calls.length).toBeGreaterThan(0);
      const { tasks } = calls[calls.length - 1][1] as { tasks: Task[] };
      // 行删除：母卡 subtasks 清空
      const motherRow = tasks.find((t) => t.id === "m1");
      expect(motherRow?.subtasks ?? []).toHaveLength(0);
      // 子卡保留但解除关联
      const childRow = tasks.find((t) => t.id === "c1");
      expect(childRow).toBeDefined();
      expect(childRow?.parentId).toBeNull();
    });
  });

  // 写冲突回归钉（真机复现：＋ 生成子卡后点 × 拒写）：
  // mutate 落盘后 tasksRef 必须换成已打戳的行，否则同一行第二次整行写
  // 的基线落后于库，RMW 守卫拒写
  it("同一母卡连续整行写（＋ 生成子卡 → × 删行）不触发 RMW 写冲突", async () => {
    const user = userEvent.setup();
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    // 模拟后端 RMW 守卫：expected 与库内现行 updated_at 不符即拒写
    const dbUpdatedAt = new Map<string, number>();
    mocks.invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "db_load") {
        return [
          {
            id: "m1",
            title: "母卡",
            column: "todo",
            order: 0,
            updatedAt: 1,
            subtasks: [{ id: "s1", text: "待生成", done: false }],
          } as Task,
        ];
      }
      if (cmd === "db_upsert") {
        for (const t of (args as { tasks: Task[] }).tasks) {
          const cur = dbUpdatedAt.get(t.id);
          if (
            t.expectedUpdatedAt !== undefined &&
            cur !== undefined &&
            cur !== t.expectedUpdatedAt
          ) {
            throw new Error(`写冲突：任务 ${t.id} 基线过期`);
          }
          dbUpdatedAt.set(t.id, t.updatedAt ?? 0);
        }
        return null;
      }
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "任务" }));
    await user.dblClick(await screen.findByText("母卡"));
    await user.click(await screen.findByTitle("生成为任务卡"));
    await waitFor(() => expect(dbUpdatedAt.has("m1")).toBe(true));
    await user.click(await screen.findByTitle("删除子任务"));
    // 第二批整行写（子卡 id 运行时生成，按批次断言）：母卡行已删 + 某行 parentId 清空
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert",
      );
      expect(calls.length).toBe(2);
      const { tasks } = calls[1][1] as { tasks: Task[] };
      const motherRow = tasks.find((t) => t.id === "m1");
      expect(motherRow?.subtasks ?? []).toHaveLength(0);
      expect(tasks.some((t) => t.parentId === null)).toBe(true);
    });
    // 写冲突会走 mutateFire 的 console.error("[mutate] persist failed") 兜底——不应出现
    expect(errSpy).not.toHaveBeenCalledWith(
      "[mutate] persist failed",
      expect.anything(),
    );
    errSpy.mockRestore();
  });

  // tasks-updated 事件合并后，规则改动（今日归位/超时归档）
  // 必须落盘，否则只改内存 → 重启读 db 回到原始数据
  it("tasks-updated 合并：超时归档的规则改动落盘 db_upsert + console 观测行", async () => {
    const handlers: Record<string, (e: unknown) => Promise<void>> = {};
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: unknown) => Promise<void>) => {
        handlers[event] = cb;
        return () => {};
      },
    );
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
    });
    expect(handlers["tasks-updated"]).toBeDefined();
    const infoSpy = vi.spyOn(console, "info").mockImplementation(() => {});
    mocks.invokeMock.mockClear();
    // 完成超过 7 天、未归档的任务 → applyArchiveRule 应标记 archived
    const staleDone: Task = {
      id: "w1",
      title: "早已完成的老任务",
      column: "done",
      completedAt: Date.now() - 8 * 24 * 60 * 60 * 1000,
      order: 99,
    };
    await act(async () => {
      await handlers["tasks-updated"]({
        payload: { upserts: [staleDone], deletes: [] },
      });
    });
    const upsertCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "db_upsert",
    );
    // 第 1 次：事件原始行落盘；第 2 次：归档规则改动落盘（修复点）
    expect(upsertCalls.length).toBe(2);
    const ruleWrite = upsertCalls[1][1] as { tasks: Task[] };
    expect(ruleWrite.tasks).toHaveLength(1);
    expect(ruleWrite.tasks[0].id).toBe("w1");
    expect(ruleWrite.tasks[0].archived).toBe(true);
    // 观测行：merge_tasks | N changed
    expect(infoSpy).toHaveBeenCalledWith(
      "[tasks-updated] merge_tasks | 1 changed",
    );
    infoSpy.mockRestore();
  });

  // delete 失败不阻断合并广播：失败行暂留 UI，
  // 后续 tasks-updated/tasks-changed 事件自愈
  it("tasks-updated delete 失败仍合并广播", async () => {
    const handlers: Record<string, (e: unknown) => Promise<void>> = {};
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: unknown) => Promise<void>) => {
        handlers[event] = cb;
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return [];
      if (cmd === "db_delete") throw new Error("delete boom");
      return null;
    });
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<App />);
    await waitFor(() => {
      expect(handlers["tasks-updated"]).toBeDefined();
    });
    await act(async () => {
      await handlers["tasks-updated"]({
        payload: {
          upserts: [
            { id: "d1", title: "删失败仍上屏", column: "todo", order: 50 },
          ],
          deletes: ["gone-1"],
        },
      });
    });
    expect(screen.getByText("删失败仍上屏")).toBeInTheDocument();
    expect(errSpy).toHaveBeenCalledWith(
      "[tasks-updated] deleteTaskRows failed",
      expect.any(Error),
    );
    errSpy.mockRestore();
  });

  // mutate 路径 delete 失败同治（ 采纳）：彻底删除走 mutate 的
  // deleteTaskRows，失败不阻断 UI 更新与后续链（失败行留库，下次 db_load 自愈回来）
  it("彻底删除 db_delete 失败 → mutate 不阻断，行从 UI 消失 + console 留痕", async () => {
    const user = userEvent.setup();
    const trashed: Task[] = [
      {
        id: "t9",
        title: "回收站里的任务",
        column: "todo",
        order: 0,
        updatedAt: 1,
        deletedAt: 123,
      },
    ];
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return trashed;
      if (cmd === "db_delete") throw new Error("delete boom");
      return null;
    });
    render(<App />);
    // 切到回收站视图（trash 态卡片不在看板渲染）
    await waitFor(() => {
      expect(screen.getByText("回收站")).toBeInTheDocument();
    });
    await user.click(screen.getByText("回收站"));
    await waitFor(() => {
      expect(screen.getByText("回收站里的任务")).toBeInTheDocument();
    });
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    await user.click(screen.getByText(/彻底删除/));
    await waitFor(() => {
      expect(errSpy).toHaveBeenCalledWith(
        "[mutate] deleteTaskRows failed",
        expect.any(Error),
      );
    });
    // UI 照常更新（mutate 未被 delete 失败阻断）：行从看板消失
    expect(screen.queryByText("回收站里的任务")).not.toBeInTheDocument();
    errSpy.mockRestore();
  });

  // tasks-updated source 守卫（mutation.rs 协议）：bot/api/migration 已由后端落盘，
  // 主窗口只合并 UI 不回写——否则异步回写用旧事件快照覆盖后端新写入（归档/软删被回滚）。
  // 回归锚：BACKEND_PERSISTED_SOURCES 若被改窄，这里立刻红。
  describe("tasks-updated source 守卫", () => {
    const setupHandler = async () => {
      const handlers: Record<string, (e: unknown) => Promise<void>> = {};
      mocks.listenMock.mockImplementation(
        async (event: string, cb: (e: unknown) => Promise<void>) => {
          handlers[event] = cb;
          return () => {};
        },
      );
      render(<App />);
      await waitFor(() => {
        expect(screen.getByText(FIXTURE_TITLE)).toBeInTheDocument();
      });
      mocks.invokeMock.mockClear();
      return handlers;
    };
    // 不触发归档/今日规则的普通 todo 行（规则改动落盘是独立路径，不在断言范围）
    const mkTask = (id: string): Task => ({
      id,
      title: `后端写入 ${id}`,
      column: "todo",
      order: 100,
    });
    const dbWriteCalls = () =>
      mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert" || c[0] === "db_delete",
      );

    it.each(["bot", "api", "migration"])(
      "source=%s（后端已落盘）→ 主窗口不回写 db",
      async (source) => {
        const handlers = await setupHandler();
        await act(async () => {
          await handlers["tasks-updated"]({
            payload: { upserts: [mkTask(`${source}1`)], deletes: [], source },
          });
        });
        expect(dbWriteCalls()).toHaveLength(0);
        // UI 仍合并：新行出现在看板
        expect(screen.getByText(`后端写入 ${source}1`)).toBeInTheDocument();
      },
    );

    it("未知 source（协议外字符串）→ WARN + 按未落盘处理仍回写", async () => {
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
      const handlers = await setupHandler();
      await act(async () => {
        await handlers["tasks-updated"]({
          payload: { upserts: [mkTask("u1")], deletes: [], source: "cron" },
        });
      });
      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining("unknown source: cron"),
      );
      expect(dbWriteCalls().length).toBeGreaterThan(0);
      warnSpy.mockRestore();
    });
  });

  // 导入后必须以 DB 为单一真源——importTasksFromFile 成功后
  // 重新 db_load 全量 → setTasks(fresh) → 再广播，而不是清空 UI 直接广播
  // （避免其他窗口读到中间态，制造"数据全丢"假象）。实现已符合，本例为回归测试。
  it("导入任务：tasks_import 成功后重新 db_load，UI 显示 fresh 数据", async () => {
    const user = userEvent.setup();
    const fresh: Task[] = [
      {
        id: "imp1",
        title: "导入的 fresh 任务",
        column: "todo",
        order: 0,
        updatedAt: 1,
      },
    ];
    let dbLoadCount = 0;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") {
        dbLoadCount++;
        // 首次：空库（不注入任何数据）；导入后：返回合并后的 fresh 数据
        return dbLoadCount === 1 ? [] : fresh;
      }
      if (cmd === "tasks_import") return 2;
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "bot_get_config")
        return { baseUrl: "", model: "", hasApiKey: false };
      if (cmd === "bot_get_enabled" || cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      return defaultInvokeImpl(cmd);
    });
    mocks.openMock.mockResolvedValue("/tmp/import.json");
    render(<App />);
    // 首屏是空库：等首次 db_load 落地即可（无注入数据可等）
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("db_load");
    });
    // 进设置页点「导入」（eefa78f 起有任务导入 + 工作区导入两个，取第一个 = 任务导入；U20B 图标化后 name 为纯文字）
    // 设置入口迁入左侧导航栏（icon + 文字「设置」，旧 ⚙️ 的 title 属性取消）
    await user.click(screen.getByText("设置"));
    //  设置壳：任务数据面板在「任务与工作区」分类下（hidden 查不到 role，先导航）
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    const importBtn = (
      await screen.findAllByRole("button", { name: "导入" })
    )[0];
    await user.click(importBtn);
    // 导入完成 alert（证明 importTasks 全流程走完）
    await waitFor(() => {
      expect(alertMock).toHaveBeenCalledWith("导入完成：本次写入 2 条任务卡");
    });
    // db_load 第二次调用必须发生在 tasks_import 之后（DB 是单一真源）
    const calls = mocks.invokeMock.mock.calls;
    const importOrder =
      mocks.invokeMock.mock.invocationCallOrder[
        calls.findIndex((c) => c[0] === "tasks_import")
      ];
    const secondLoadIdx = calls.reduce(
      (idx, c, i) => (c[0] === "db_load" && i > 0 ? i : idx),
      -1,
    );
    expect(secondLoadIdx).toBeGreaterThan(-1);
    expect(
      mocks.invokeMock.mock.invocationCallOrder[secondLoadIdx],
    ).toBeGreaterThan(importOrder);
    // setTasks 收到 fresh 数据：点设置壳「返回」回看板（fresh 任务在、导入前的库内任务已被 DB 真值取代）
    await user.click(screen.getByRole("button", { name: /返回/ }));
    expect(await screen.findByText("导入的 fresh 任务")).toBeInTheDocument();
    expect(
      screen.queryByText(FIXTURE_TITLE),
    ).not.toBeInTheDocument();
  });
});
