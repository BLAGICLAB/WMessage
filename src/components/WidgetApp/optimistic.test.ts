// patchTaskOptimistic 单测：钉住「snapshot prev → map 应用 patch → 写 ref + setTasks
// → invoke task_patch {id, patch} 嵌套 → on err handleCommandError + 回滚 prev」契约。
// 测 helper 内部回滚（ref + setTasks 都还原 prev），不再验 WidgetApp 端
// rollbackOptimistic 闭包——helper 抽取后那条闭包只服务 task_set_column（toggleDone），
// 自己的回滚职责在 helper 内重新实现。
import { describe, expect, it, vi, beforeEach } from "vitest";
import type { Task } from "../../types";

const invokeMock = vi.hoisted(() => vi.fn());
const handleCommandErrorMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../../lib/errorHandler", () => ({
  handleCommandError: handleCommandErrorMock,
}));

// vi.mock 必须在被测模块 import 之前注册——动态 import 拿最新 module graph
const importHelper = () => import("./optimistic");

const mkTask = (over: Partial<Task> = {}): Task => ({
  id: "t1",
  title: "原标题",
  column: "todo",
  ...over,
});

/** 重新解析回调（create ref 模拟 useRef<Task[]>）；setTasks 用 push 记录所有调用 */
const setup = (initial: Task[]) => {
  const calls: Task[][] = [];
  const tasksRef = { current: initial } as React.MutableRefObject<Task[]>;
  const setTasks = (t: Task[]) => calls.push(t);
  return { tasksRef, setTasks, calls };
};

beforeEach(() => {
  invokeMock.mockReset();
  handleCommandErrorMock.mockReset();
});

describe("patchTaskOptimistic 成功路径", () => {
  it("ref + state 即时反映 patch；invoke 收到嵌套 {id, patch}", async () => {
    invokeMock.mockResolvedValue({});
    const { patchTaskOptimistic } = await importHelper();
    const prev = [mkTask(), mkTask({ id: "t2", title: "其他" })];
    const { tasksRef, setTasks, calls } = setup(prev);

    patchTaskOptimistic(
      tasksRef,
      setTasks,
      "t1",
      { title: "新标题", collapsed: true },
      "更新任务",
    );

    // state / ref 都写成乐观结果（只命中行被合并 patch）
    expect(tasksRef.current).toHaveLength(2);
    expect(tasksRef.current[0]).toMatchObject({
      id: "t1",
      title: "新标题",
      collapsed: true,
    });
    expect(tasksRef.current[1]).toEqual({ id: "t2", title: "其他", column: "todo" });
    // setTasks 至少收到乐观结果（不依赖次数，只验形状）
    expect(calls[calls.length - 1]).toEqual(tasksRef.current);
    // invoke 收到 task_patch 命令 + 嵌套 {id, patch}（线协议必须嵌套，不是铺平）
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("task_patch", {
      id: "t1",
      patch: { title: "新标题", collapsed: true },
    });
    // 成功路径不调 handleCommandError
    expect(handleCommandErrorMock).not.toHaveBeenCalled();
  });
});

describe("patchTaskOptimistic 失败路径", () => {
  it("invoke reject → handleCommandError + 还原 ref 与 setTasks 到 prev", async () => {
    invokeMock.mockRejectedValue(new Error("boom"));
    const { patchTaskOptimistic } = await importHelper();
    const prev = [mkTask({ id: "t1" }), mkTask({ id: "t2" })];
    const { tasksRef, setTasks, calls } = setup(prev);

    patchTaskOptimistic(tasksRef, setTasks, "t1", { title: "新" }, "更新任务");

    // 乐观已写入（calls 控制返回 = tasksRef.current 应是新对象）
    expect(calls).toHaveLength(1);
    expect(tasksRef.current[0].title).toBe("新");
    // 等 microtask 让 catch 落地
    await Promise.resolve();
    await Promise.resolve();

    // 回滚：ref + 后续 setTasks 都被还原到 prev 引用
    expect(handleCommandErrorMock).toHaveBeenCalledTimes(1);
    expect(handleCommandErrorMock).toHaveBeenCalledWith(
      expect.any(Error),
      "更新任务",
    );
    expect(tasksRef.current).toBe(prev);
    // setTasks 被再调一次，参数 = prev（与首调乐观结果形状不同）
    expect(calls).toHaveLength(2);
    expect(calls[1]).toBe(prev);
  });

  it("ref 已被外部覆盖（非乐观结果）→ 不回滚（避免用失败前快照盖掉更新）", async () => {
    invokeMock.mockRejectedValue(new Error("boom"));
    const { patchTaskOptimistic } = await importHelper();
    const prev = [mkTask({ id: "t1" })];
    const { tasksRef, setTasks, calls } = setup(prev);

    patchTaskOptimistic(tasksRef, setTasks, "t1", { title: "新" }, "更新任务");

    // 模拟「乐观写入后、catch 触发前」外部已用 tasks-changed 收敛把 ref 改写成更新版
    const externalUpdate = [mkTask({ id: "t1", title: "服务端更新" })];
    tasksRef.current = externalUpdate;

    await Promise.resolve();
    await Promise.resolve();

    expect(handleCommandErrorMock).toHaveBeenCalledTimes(1);
    // ref 不被回滚（仍是外部收敛后的 externalUpdate）
    expect(tasksRef.current).toBe(externalUpdate);
    // setTasks 也不再被回滚调用
    expect(calls).toHaveLength(1);
  });
});