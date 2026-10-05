import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  Controls,
  useReactFlow,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Download, Loader2, Play, Plus, RefreshCw, Save, Square, Trash2, Upload } from "lucide-react";
import { handleCommandError } from "../../lib/errorHandler";
import { getDecomposeGuidance } from "../../lib/workflowPrompt";
import type { Task, Workflow, WorkflowSaveResult } from "../../types";
import {
  draftFromDecompose,
  draftFromTasks,
  wouldCreateCycle,
  type CanvasNode,
} from "./graph";
import { TaskNode, type TaskNodeData } from "./TaskNode";
import { GoalNode, type GoalNodeData } from "./GoalNode";

/** 总目标卡在画布上的固定节点 id（绑定 workflows 元数据，非任务卡） */
const GOAL_ID = "__goal__";
/** 拆解自动命名的截取长度（OCR r1 low：魔法数字提升为具名常量） */
const NAME_AUTO_LEN = 12;

/** 开始/继续执行按钮的 title（OCR r1 high：拆掉嵌套三元；W5 r1：dirty 优先级最高，
 *  续跑提示不得吞掉"先保存"警告） */
function runButtonTitle(
  dirty: boolean,
  hasActive: boolean,
  doneCount: number
): string {
  if (dirty) return "先保存再执行";
  if (!hasActive) return "先选择或保存一个工作流";
  if (doneCount > 0)
    return `继续执行：已完成 ${doneCount} 个节点将跳过（断点续跑），失败/未跑的重新执行`;
  return "执行整张图（已完成节点自动跳过 = 断点续跑）";
}

const nodeTypes = { task: TaskNode, goal: GoalNode };

export function WorkflowPage({
  tasks,
  onSetColumn,
  onUpdate,
  onTasksReload,
}: {
  /** App 全量任务（含 origin=workflow——节点状态渲染源） */
  tasks: Task[];
  onSetColumn: (taskId: string, col: "todo" | "doing" | "done") => void;
  onUpdate: (taskId: string, patch: Partial<Task>) => void;
  /** 保存/删除后重读全量任务（App 侧统一套规则 + 广播） */
  onTasksReload: () => void;
}) {
  return (
    <ReactFlowProvider>
      <WorkflowPageInner
        tasks={tasks}
        onSetColumn={onSetColumn}
        onUpdate={onUpdate}
        onTasksReload={onTasksReload}
      />
    </ReactFlowProvider>
  );
}

function WorkflowPageInner({
  tasks,
  onSetColumn,
  onUpdate,
  onTasksReload,
}: {
  tasks: Task[];
  onSetColumn: (taskId: string, col: "todo" | "doing" | "done") => void;
  onUpdate: (taskId: string, patch: Partial<Task>) => void;
  onTasksReload: () => void;
}) {
  const { screenToFlowPosition } = useReactFlow();
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  /** hero = 空态引导；edit = 画布编辑 */
  const [mode, setMode] = useState<"hero" | "edit">("hero");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [goal, setGoal] = useState("");
  const [nodes, setNodes] = useState<CanvasNode[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [savedSnapshot, setSavedSnapshot] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** 两步确认： armed 的 workflowId，第二次点击才真删 */
  const [deleteArmed, setDeleteArmed] = useState(false);
  /** 最新 props 镜像：useCallback 闭包里读 tasksRef 而非捕获 tasks，防过期（OCR r1 high）。
   *  经 useEffect 同步（render 期写 ref 会被 lint 拦；回调只在交互后触发，晚一拍无碍） */
  const tasksRef = useRef(tasks);
  const propsRef = useRef({ onSetColumn, onUpdate });
  useEffect(() => {
    tasksRef.current = tasks;
    propsRef.current = { onSetColumn, onUpdate };
  });
  /** 打开竞态守卫：慢的旧 workflow_load 响应不得覆盖用户后来的选择（OCR r1 medium） */
  const openSeqRef = useRef(0);
  /** 删除确认的 3s 复位定时器（卸载/重臂时清理，OCR r1 medium） */
  const armedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 重新生成两步确认 */
  const [regenArmed, setRegenArmed] = useState(false);
  /** 工作流执行中（W3-RUNNER）：开始执行/停止按钮切换 + 进度显示 */
  const [running, setRunning] = useState(false);
  /** 模型库条目（W6-MODEL）：节点执行模型下拉选项 */
  const [models, setModels] = useState<Array<{ id: string; label: string }>>([]);
  /** 拆解附件路径（W8-ATTACH）：随工作流持久化（重新生成可复用） */
  const [attachPaths, setAttachPaths] = useState<string[]>([]);
  /** AI 拆解进行中 + 竞态守卫（取消 = 递增序号丢弃在途响应） */
  const [decomposing, setDecomposing] = useState(false);
  const decomposeSeqRef = useRef(0);
  /** 名称是否仍是自动名（true 时拆解成功用 goal 前缀重命名；用户改过名则尊重用户） */
  const [nameAuto, setNameAuto] = useState(true);
  useEffect(
    () => () => {
      decomposeSeqRef.current++; // 卸载时使在途拆解响应失效
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    },
    []
  );

  // 工作流列表（进入页面拉一次）
  useEffect(() => {
    invoke<Workflow[]>("workflow_list")
      .then(setWorkflows)
      .catch((e) => handleCommandError(e, "读取工作流列表", { silent: true }));
  }, []);
  // 模型库条目（W6-MODEL）：每卡执行模型下拉
  useEffect(() => {
    invoke<{
      modelsByProvider?: {
        openai?: Array<{ id: string; label: string; enabled?: boolean }>;
        anthropic?: Array<{ id: string; label: string; enabled?: boolean }>;
      } | null;
    }>("bot_get_config")
      .then((c) => {
        const m = c.modelsByProvider;
        // 停用条目不进下拉（与聊天面板 enabled 过滤同规则）
        const list = [...(m?.openai ?? []), ...(m?.anthropic ?? [])]
          .filter((e) => e.enabled !== false)
          .map((e) => ({ id: e.id, label: e.label }));
        setModels(list);
      })
      .catch((e) => handleCommandError(e, "读取模型库", { silent: true }));
  }, []);

  const snapshot = useCallback(
    () => JSON.stringify({ name, goal, nodes, attachPaths }),
    [name, goal, nodes, attachPaths]
  );
  const dirty = savedSnapshot !== snapshot();

  // ────────────── 打开/新建 ──────────────

  const openWorkflow = async (id: string) => {
    const seq = ++openSeqRef.current;
    decomposeSeqRef.current++; // 使在途拆解响应失效（OCR r1 critical）
    try {
      const detail = await invoke<
        Workflow & { tasks: Task[]; attachments?: string[] | null }
      >("workflow_load", { id });
      if (seq !== openSeqRef.current) return; // 期间用户已切换：丢弃本次响应
      setAttachPaths(detail.attachments ?? []);
      const fresh = draftFromTasks(detail.tasks);
      setActiveId(id);
      setName(detail.name);
      setGoal(detail.goal);
      setNodes(fresh);
      setSelectedIds([]);
      setSavedSnapshot(
        JSON.stringify({
          name: detail.name,
          goal: detail.goal,
          nodes: fresh,
          attachPaths: detail.attachments ?? [],
        })
      );
      setNameAuto(false); // 打开的是已保存工作流：名称是作者起的，拆解不得覆盖（OCR r2）
      setRunning(false); // 先复位：A 在跑时切到 B，停止按钮不得跨工作流残留（全量对照 high）
      const iseq = openSeqRef.current;
      invoke<boolean>("workflow_is_running", { workflowId: id })
        .then((v) => {
          // 慢响应不得覆盖后续切换（OCR r1 high）
          if (iseq === openSeqRef.current) setRunning(v);
        })
        .catch(() => {});
      setMode("edit");
      setDeleteArmed(false);
      setRegenArmed(false);
    } catch (e) {
      handleCommandError(e, "打开工作流", { onRetry: () => void openWorkflow(id) });
    }
  };

  const createBlank = () => {
    openSeqRef.current++; // 使在途的 workflow_load 失效
    decomposeSeqRef.current++; // 同上（OCR r1 critical）
    setActiveId(null);
    setName(`工作流 ${new Date().toLocaleDateString()}`);
    setGoal("");
    setNodes([]);
    setSelectedIds([]);
    setSavedSnapshot(null);
    setMode("edit");
    setDeleteArmed(false);
    // regenArmed/running 必须随画布销毁复位（W4 r1：regenArmed 残留会让下次
    // 一次点击即触发生成；running 残留会让新画布显示停止按钮）
    setRegenArmed(false);
    setRunning(false);
    setAttachPaths([]);
  };

  // ────────────── 模板导入导出（W4-TEMPLATE，设计 §4） ──────────────

  const doExport = async () => {
    if (!activeId) return;
    try {
      const path = await saveDialog({
        defaultPath: `${name.trim() || "工作流"}.wflow.json`,
        filters: [{ name: "WMessage 工作流", extensions: ["json"] }],
      });
      if (!path) return; // 用户取消
      const count = await invoke<number>("workflow_export", {
        workflowId: activeId,
        path,
      });
      alert(`导出完成：共 ${count} 个节点（导出的是已保存版本）`);
    } catch (e) {
      handleCommandError(e, "导出工作流模板", { onRetry: () => void doExport() });
    }
  };

  const doImport = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "WMessage 工作流", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return; // 用户取消
      const res = await invoke<WorkflowSaveResult>("workflow_import", {
        path: selected,
      });
      // 先打开新实例再提示（OCR r1 high：顺序颠倒会在 openWorkflow 失败时
      // 出现"已成功 + 重试又导入一份"的悖论）；列表刷新失败走既有 silent 弹窗模式
      await openWorkflow(res.workflowId);
      invoke<Workflow[]>("workflow_list")
        .then(setWorkflows)
        .catch((e) => handleCommandError(e, "读取工作流列表", { silent: true }));
      alert(`导入完成：实例化 ${res.created} 个节点（全新副本，与原模板互不影响）`);
    } catch (e) {
      handleCommandError(e, "导入工作流模板", { onRetry: () => void doImport() });
    }
  };

  // ────────────── AI 拆解（W2-DECOMPOSE，设计 §6） ──────────────

  const runDecompose = async (goalText: string) => {
    if (decomposing) return;
    const seq = ++decomposeSeqRef.current;
    setDecomposing(true);
    try {
      const res = await invoke<{
        subtasks: Array<{ title: string; note: string | null; dependsOn: number[] }>;
        attempts: number;
      }>("workflow_decompose", {
        goal: goalText,
        guidance: getDecomposeGuidance(),
        attachments: attachPaths.length ? attachPaths : null,
      });
      if (seq !== decomposeSeqRef.current) return; // 已取消/已卸载：丢弃响应
      const fresh = draftFromDecompose(res.subtasks);
      setNodes(fresh);
      setSelectedIds([]);
      if (nameAuto) setName(goalText.slice(0, NAME_AUTO_LEN));
      setSavedSnapshot(null); // 拆解结果 = 新草稿，保存才落库（设计 §7）
      setMode("edit");
    } catch (e) {
      if (seq === decomposeSeqRef.current) {
        handleCommandError(e, "AI 拆解", { onRetry: () => void runDecompose(goalText) });
      }
    } finally {
      if (seq === decomposeSeqRef.current) setDecomposing(false);
    }
  };

  const addAttachments = async () => {
    try {
      const picked = await open({
        multiple: true,
        directory: false,
      });
      const list = typeof picked === "string" ? [picked] : (picked ?? []);
      if (!list.length) return;
      setAttachPaths((prev) => [...prev, ...list.filter((p) => !prev.includes(p))]);
    } catch (e) {
      handleCommandError(e, "添加附件");
    }
  };

  const cancelDecompose = () => {
    decomposeSeqRef.current++; // 在途响应作废（v1 语义：忽略结果，非中断请求）
    setDecomposing(false);
  };

  const regenerate = () => {
    if (decomposing) return;
    if (!regenArmed) {
      setRegenArmed(true);
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
      armedTimerRef.current = setTimeout(() => setRegenArmed(false), 3000);
      return;
    }
    if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    setRegenArmed(false);
    // 回输入框预填原目标（设计 §5.3）；activeId 保留——再次保存时按指纹 diff 替换
    setMode("hero");
  };

  // ────────────── 节点编辑 ──────────────

  const addNode = () => {
    decomposeSeqRef.current++; // 手动加卡 = 放弃在途拆解结果（OCR r1 medium）
    setRegenArmed(false);
    // 视口中心落点（screenToFlowPosition 需画布 DOM 存在；空画布也有容器）
    const center = screenToFlowPosition({
      x: window.innerWidth / 2,
      y: window.innerHeight / 2,
    });
    const n: CanvasNode = {
      localId: crypto.randomUUID(),
      title: "新任务",
      dependsOn: [],
      pos: { x: center.x - 170, y: center.y - 75 },
    };
    setNodes((prev) => [...prev, n]);
  };

  const deleteNode = useCallback((localId: string) => {
    // 连带清理：删掉它自己 + 所有人对它的依赖（设计 §决策8）
    setNodes((prev) =>
      prev
        .filter((n) => n.localId !== localId)
        .map((n) => ({ ...n, dependsOn: n.dependsOn.filter((d) => d !== localId) }))
    );
    setSelectedIds((prev) => prev.filter((s) => s !== localId));
  }, []);

  const onNodesChange = useCallback((changes: NodeChange[]) => {
    setNodes((prev) => {
      let next = prev;
      for (const c of changes) {
        if (c.type === "position" && c.position) {
          next = next.map((n) =>
            n.localId === c.id ? { ...n, pos: c.position! } : n
          );
        } else if (c.type === "remove") {
          next = next
            .filter((n) => n.localId !== c.id)
            .map((n) => ({
              ...n,
              dependsOn: n.dependsOn.filter((d) => d !== c.id),
            }));
        }
      }
      return next;
    });
    setSelectedIds((prev) => {
      let next = prev;
      for (const c of changes) {
        if (c.type === "select") {
          next = c.selected
            ? [...next, c.id]
            : next.filter((s) => s !== c.id);
        } else if (c.type === "remove") {
          // 节点被键盘/程序删除时同步清理选中态，防悬空 id（OCR r1 medium）
          next = next.filter((s) => s !== c.id);
        }
      }
      return next;
    });
  }, []);

  const isValidConnection = useCallback(
    (c: Edge | Connection) => {
      if (!c.source || !c.target) return false;
      const target = nodes.find((n) => n.localId === c.target);
      if (!target) return false;
      if (target.dependsOn.includes(c.source)) return false;
      return !wouldCreateCycle(nodes, c.source, c.target);
    },
    [nodes]
  );

  const onConnect = useCallback(
    (c: Connection) => {
      if (!c.source || !c.target || !isValidConnection(c)) return;
      setNodes((prev) =>
        prev.map((n) =>
          n.localId === c.target && !n.dependsOn.includes(c.source!)
            ? { ...n, dependsOn: [...n.dependsOn, c.source!] }
            : n
        )
      );
    },
    [isValidConnection]
  );

  const onEdgesChange = useCallback((changes: EdgeChange[]) => {
    setNodes((prev) => {
      let next = prev;
      for (const c of changes) {
        if (c.type === "remove") {
          // edge id = `${source}->${target}`；删边 = 下游 dependsOn 移除上游。
          // id 格式约定：localId 恒为 uuid/真实任务 id（不含 "->"），本文件内
          // 生成与解析一一对应（rfEdges useMemo 与此处），不外溢成通用格式
          const [, target] = c.id.split("->");
          next = next.map((n) =>
            n.localId === target
              ? {
                  ...n,
                  dependsOn: n.dependsOn.filter(
                    (d) => `${d}->${n.localId}` !== c.id
                  ),
                }
              : n
          );
        }
      }
      return next;
    });
  }, []);

  // ────────────── 已保存节点的任务卡操作（同步草稿 + 真实任务） ──────────────
  // 一律经 ref 读最新 props/tasks，避免闭包过期被 React Flow 的节点 data 缓存
  // （OCR r1 high：rfNodes useMemo 不依赖回调身份，回调必须自身稳定）

  /** 模型切换：草稿与真实任务同写（W6 r1 high：只 patch 不写草稿会在保存时回退） */
  const changeModel = useCallback(
    (taskId: string, model: string | undefined) => {
      setNodes((prev) =>
        prev.map((n) => (n.taskId === taskId ? { ...n, model } : n))
      );
      propsRef.current.onUpdate(taskId, model ? { model } : { model: undefined });
    },
    []
  );

  const commitTitle = useCallback((taskId: string, title: string) => {
    setNodes((prev) =>
      prev.map((n) => (n.taskId === taskId ? { ...n, title } : n))
    );
    propsRef.current.onUpdate(taskId, { title });
  }, []);

  const toggleDone = useCallback((taskId: string) => {
    const t = tasksRef.current.find((x) => x.id === taskId);
    if (!t) return;
    propsRef.current.onSetColumn(taskId, t.column === "done" ? "todo" : "done");
  }, []);

  const toggleSubtask = useCallback((taskId: string, subtaskId: string) => {
    const t = tasksRef.current.find((x) => x.id === taskId);
    if (!t?.subtasks) return;
    propsRef.current.onUpdate(taskId, {
      subtasks: t.subtasks.map((s) =>
        s.id === subtaskId ? { ...s, done: !s.done } : s
      ),
    });
  }, []);

  // ────────────── 执行（W3-RUNNER，设计 §8） ──────────────

  const startBusyRef = useRef(false);
  const startRun = async () => {
    if (!activeId || running || startBusyRef.current) return;
    // 未保存修改先拦下（保存键未点时节点卡不存在）
    if (dirty) {
      handleCommandError(new Error("先保存再执行：未保存的草稿还没有落库节点卡"), "开始执行");
      return;
    }
    startBusyRef.current = true;
    const seq = openSeqRef.current;
    try {
      // running 置真放在成功后：避免后端尚未登记时轮询提前开跑（OCR r1 medium）；
      // seq 守卫：执行期间用户新建/切换画布，晚到响应不得置新画布为执行态（OCR r1）
      await invoke("workflow_run", { workflowId: activeId });
      if (seq !== openSeqRef.current) return;
      setRunning(true);
    } catch (e) {
      setRunning(false);
      handleCommandError(e, "开始执行", { onRetry: () => void startRun() });
    } finally {
      startBusyRef.current = false;
    }
  };

  const stopBusyRef = useRef(false);
  const stopRun = async () => {
    const wf = activeId;
    if (!wf || stopBusyRef.current) return;
    stopBusyRef.current = true;
    try {
      await invoke("workflow_stop", { workflowId: wf });
      // 300ms 后回查复位（OCR r2）：闭包绑定被停的 wf；seq 快照保证用户已
      // 切走时本回查不应用（切换路径各有自己的 running 取真/归零）
      const seq = openSeqRef.current;
      setTimeout(() => {
        if (seq !== openSeqRef.current) return;
        invoke<boolean>("workflow_is_running", { workflowId: wf })
          .then((v) => {
            if (seq === openSeqRef.current) setRunning(v);
          })
          .catch(() => {});
      }, 300);
    } catch (e) {
      handleCommandError(e, "停止工作流");
    } finally {
      // busy 推迟到回查窗口之后解除，防回查期间重复点击（OCR r2）
      setTimeout(() => {
        stopBusyRef.current = false;
      }, 700);
    }
  };

  // 执行态轮询：running 时每 5s 问一次后端（控制台注销即按钮复位）
  useEffect(() => {
    if (!running || !activeId) return;
    let failures = 0;
    const t = setInterval(() => {
      invoke<boolean>("workflow_is_running", { workflowId: activeId })
        .then((v) => {
          failures = 0;
          setRunning(v);
        })
        .catch((e) => {
          // 连续 3 次查询失败 → 复位按钮（持续轮询无意义，OCR r1）
          failures += 1;
          if (failures >= 3) {
            console.error("[workflow] 执行态查询连续失败，复位按钮", e);
            setRunning(false);
          }
        });
    }, 5000);
    return () => clearInterval(t);
  }, [running, activeId]);

  // ────────────── 保存（指纹 diff 落库，设计 §7） ──────────────

  const save = async () => {
    if (saving) return;
    setSaving(true);
    // 后端的 name/goal 会 trim，本地 state 同步成 trim 后的值——
    // 否则保存后 snapshot 用原值算 dirty 恒为 true（OCR r1 medium）
    const effectiveName = name.trim() || "未命名工作流";
    const effectiveGoal = goal.trim() || effectiveName;
    try {
      const res = await invoke<WorkflowSaveResult>("workflow_save", {
        input: {
          workflowId: activeId,
          name: effectiveName,
          goal: effectiveGoal,
          attachments: attachPaths.length ? attachPaths : null,
          nodes: nodes.map((n) => ({
            localId: n.localId,
            taskId: n.taskId ?? null,
            title: n.title,
            note: n.note ?? null,
            tags: n.tags ?? null,
            dependsOn: n.dependsOn,
            pos: n.pos,
            // 保存链必须携带 model（W6 r1 critical：缺失会把下拉刚设的模型置空）
            model: n.model ?? tasks.find((t) => t.id === n.taskId)?.model ?? null,
            subtasks: n.subtasks ?? null,
          })),
        },
      });
      // 重建画布绑定：localId → 真实任务 id（新卡 uuid 由服务端分配），
      // dependsOn 同步重映射，保持「已保存后 localId === taskId」不变量
      const remap = new Map(res.bindings.map((b) => [b.localId, b.taskId]));
      const next = nodes.map((n) => {
        const tid = remap.get(n.localId) ?? n.taskId ?? n.localId;
        const mapped: CanvasNode = {
          ...n,
          localId: tid,
          taskId: tid,
          dependsOn: n.dependsOn.map((d) => remap.get(d) ?? d),
        };
        return mapped;
      });
      setNodes(next);
      setName(effectiveName);
      setGoal(effectiveGoal);
      setSavedSnapshot(
        JSON.stringify({
          name: effectiveName,
          goal: effectiveGoal,
          nodes: next,
          attachPaths,
        })
      );
      setActiveId(res.workflowId);
      setSelectedIds([]);
      onTasksReload();
      // 刷新工作流列表（新工作流进入下拉）
      invoke<Workflow[]>("workflow_list")
        .then(setWorkflows)
        .catch(() => {});
    } catch (e) {
      handleCommandError(e, "保存工作流", { onRetry: () => void save() });
    } finally {
      setSaving(false);
    }
  };

  const deleteWorkflow = async () => {
    if (!activeId) return;
    if (!deleteArmed) {
      setDeleteArmed(true);
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
      armedTimerRef.current = setTimeout(() => setDeleteArmed(false), 3000);
      return;
    }
    if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    setDeleteArmed(false);
    try {
      await invoke("workflow_delete", { id: activeId });
      setWorkflows((prev) => prev.filter((w) => w.id !== activeId));
      createBlank();
      onTasksReload();
    } catch (e) {
      handleCommandError(e, "删除工作流", { onRetry: () => void deleteWorkflow() });
    }
  };

  // ────────────── React Flow 数据派生 ──────────────

  // 已完成节点数（W5：驱动「继续执行」按钮态 + 总目标卡进度，单一数据源）
  const doneCount =
    activeId !== null
      ? tasks.filter((t) => t.workflowId === activeId && t.column === "done").length
      : 0;

  const rfNodes = useMemo<Node<TaskNodeData | GoalNodeData>[]>(
    () => [
      {
        id: GOAL_ID,
        type: "goal",
        position: { x: 40, y: 40 },
        draggable: false,
        selectable: false,
        deletable: false,
        data: {
          name,
          goal,
          saved: savedSnapshot !== null,
          progress: running ? { done: doneCount, total: nodes.length } : null,
          onRename: (v: string) => {
            setName(v);
            setNameAuto(false);
          },
          onGoalChange: setGoal,
        },
      },
      ...nodes.map((n) => {
        const task = n.taskId ? tasks.find((t) => t.id === n.taskId) : undefined;
        const data: TaskNodeData = {
          task,
          models,
          draftTitle: n.title,
          onDraftTitleCommit: (localId, title) =>
            setNodes((prev) =>
              prev.map((m) => (m.localId === localId ? { ...m, title } : m))
            ),
          onDelete: deleteNode,
          onToggleDone: task ? toggleDone : undefined,
          onCommitTitle: task ? commitTitle : undefined,
          onToggleSubtask: task ? toggleSubtask : undefined,
          onModelChange: task ? changeModel : undefined,
        };
        return {
          id: n.localId,
          type: "task",
          position: n.pos,
          selected: selectedIds.includes(n.localId),
          data,
        };
      }),
    ],
    // 依赖含全部 data 回调（deleteNode/toggle* 均为 useCallback 稳定引用，
    // 内部经 ref 读最新 tasks/props——此处完整列出是防过期闭包的兜底，OCR r1 high）
    [nodes, tasks, name, goal, savedSnapshot, selectedIds, deleteNode, toggleDone, commitTitle, toggleSubtask, running, activeId, doneCount, models, changeModel]
  );

  const rfEdges = useMemo<Edge[]>(
    () => {
      const ids = new Set(nodes.map((n) => n.localId));
      return nodes.flatMap((n) =>
        n.dependsOn
          .filter((d) => ids.has(d))
          .map((d) => ({
            id: `${d}->${n.localId}`,
            source: d,
            target: n.localId,
          }))
      );
    },
    [nodes]
  );

  // ────────────── 渲染 ──────────────

  const toolbarBtn =
    "flex h-8 items-center gap-1.5 whitespace-nowrap rounded-[var(--r-sm)] px-3 text-sm text-[var(--t3)] nm-outset disabled:cursor-not-allowed disabled:opacity-50";

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col">
      {/* 顶栏：工作流切换 + 工具栏（设计 §5.1） */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 pb-3">
        <select
          aria-label="切换工作流"
          className="h-8 rounded-[var(--r-sm)] bg-transparent px-2 text-sm text-[var(--t2)] nm-outset"
          value={activeId ?? ""}
          onChange={(e) => {
            const v = e.target.value;
            if (v) void openWorkflow(v);
            else createBlank();
          }}
        >
          <option value="">（未选择）</option>
          {workflows.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
        </select>
        <button
          className={toolbarBtn}
          onClick={() => setMode("hero")}
          title="新建工作流（空白画布或 AI 生成）"
        >
          <Plus size={14} aria-hidden /> 新建
        </button>
        <div className="flex-1" />
        <button className={toolbarBtn} onClick={addNode}>
          <Plus size={14} aria-hidden /> 加卡
        </button>
        <button
          className={`${toolbarBtn} ${regenArmed ? "nm-inset text-[var(--t1)]" : ""}`}
          aria-pressed={regenArmed}
          onClick={regenerate}
          disabled={decomposing || nodes.length === 0}
          title={
            regenArmed
              ? "再点一次确认回到目标输入（当前画布在保存前保持不变）"
              : "修改目标后重新拆解（当前画布在保存前保持不变）"
          }
        >
          <RefreshCw size={14} aria-hidden /> {regenArmed ? "确认重生成？" : "重新生成"}
        </button>
        <button
          className={toolbarBtn}
          onClick={() => void doImport()}
          title="从 .wflow.json 导入（实例化为全新工作流）"
        >
          <Upload size={14} aria-hidden /> 导入
        </button>
        <button
          className={toolbarBtn}
          onClick={() => void doExport()}
          disabled={activeId === null}
          title={activeId === null ? "先选择一个工作流" : "导出已保存版本为 .wflow.json 模板"}
        >
          <Download size={14} aria-hidden /> 导出
        </button>
        <button
          className={toolbarBtn}
          onClick={() => void save()}
          disabled={saving || !dirty}
          title={dirty ? "保存工作流（指纹 diff，保留未变更节点的执行痕迹）" : "没有未保存的修改"}
        >
          <Save size={14} aria-hidden /> 保存{dirty ? " •" : ""}
        </button>
        {running ? (
          <button
            className={`${toolbarBtn} nm-inset text-[var(--danger,#ef4444)]`}
            onClick={() => void stopRun()}
            title="停止执行：未启动的节点将被跳过；运行中的会话请到挂件聊天区 ■ 停止"
          >
            <Square size={14} aria-hidden /> 停止
          </button>
        ) : (
          <button
            className={toolbarBtn}
            onClick={() => void startRun()}
            disabled={activeId === null || dirty || nodes.length === 0}
            title={runButtonTitle(dirty, activeId !== null, doneCount)}
          >
            <Play size={14} aria-hidden /> {doneCount > 0 ? "继续执行" : "开始执行"}
          </button>
        )}
        {activeId && (
          <button
            className={`${toolbarBtn} ${deleteArmed ? "nm-inset text-[var(--danger,#ef4444)]" : ""}`}
            aria-pressed={deleteArmed}
            onClick={() => void deleteWorkflow()}
            title={deleteArmed ? "再点一次确认删除（连带删除全部节点卡）" : "删除当前工作流"}
          >
            <Trash2 size={14} aria-hidden /> {deleteArmed ? "确认删除？" : "删除"}
          </button>
        )}
      </div>
      {/* 画布（编辑态）；hero 态显示引导（设计 §5.1） */}
      <div className="min-h-0 flex-1">
        {mode === "hero" ? (
          <EmptyHero
            goal={goal}
            onGoalChange={setGoal}
            workflows={workflows}
            onOpen={(id) => void openWorkflow(id)}
            decomposing={decomposing}
            onDecompose={() => void runDecompose(goal)}
            onCancelDecompose={cancelDecompose}
            isRegenerate={activeId !== null}
            attachPaths={attachPaths}
            onAddAttachments={() => void addAttachments()}
            onRemoveAttachment={(p) =>
              setAttachPaths((prev) => prev.filter((x) => x !== p))
            }
          />
        ) : (
          <ReactFlow
            nodes={rfNodes}
            edges={rfEdges}
            nodeTypes={nodeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            isValidConnection={isValidConnection}
            fitView
            deleteKeyCode={["Backspace", "Delete"]}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={24} size={1.5} />
            <Controls showInteractive={false} />
          </ReactFlow>
        )}
      </div>
    </div>
  );
}

/** 空态引导（设计 §5.1 空态）：一句话目标 → AI 生成（W2 点亮）/ 创建空白 + 已有工作流列表。
 *  重新生成复用本组件：工具栏「重新生成」回到此态，goal 预填原目标 */
function EmptyHero({
  goal,
  onGoalChange,
  workflows,
  onOpen,
  decomposing,
  onDecompose,
  onCancelDecompose,
  isRegenerate,
  attachPaths,
  onAddAttachments,
  onRemoveAttachment,
}: {
  goal: string;
  onGoalChange: (v: string) => void;
  workflows: Workflow[];
  onOpen: (id: string) => void;
  decomposing: boolean;
  onDecompose: () => void;
  onCancelDecompose: () => void;
  /** 重新生成流程中（activeId 已存在）——按钮文案区分 */
  isRegenerate: boolean;
  /** 拆解附件路径（W8-ATTACH）：AI 先读附件内容再拆解 */
  attachPaths: string[];
  onAddAttachments: () => void;
  onRemoveAttachment: (path: string) => void;
}) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="nm-card w-full max-w-xl p-6">
        <h2 className="text-base font-semibold text-[var(--t1)]">
          {isRegenerate ? "重新生成工作流" : "新建工作流"}
        </h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          {isRegenerate
            ? "修改目标描述后重新生成——点「保存」前原画布保持不变，保存时按内容指纹保留未变更节点的执行痕迹。"
            : "描述你想让 AI 帮你完成的工作流程，可添加参考附件（docx/pdf/txt 等），AI 将先读附件再拆解成任务卡并自动连线。"}
        </p>
        <textarea
          aria-label="工作流目标描述"
          className="mt-4 h-24 w-full resize-none rounded-[var(--r-sm)] nm-inset p-3 text-sm text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
          placeholder="例：每周五收集本周完成的任务，汇总成一份周报文档并绑定到任务卡"
          value={goal}
          onChange={(e) => onGoalChange(e.target.value)}
          disabled={decomposing}
        />
        {attachPaths.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {attachPaths.map((p) => (
              <span
                key={p}
                className="flex max-w-full items-center gap-1 rounded-full border border-[var(--edge)] px-2.5 py-0.5 text-[11px] text-[var(--t3)]"
                title={p}
              >
                <span className="truncate">{p.split("/").pop()}</span>
                <button
                  aria-label={`移除附件 ${p.split("/").pop()}`}
                  onClick={() => onRemoveAttachment(p)}
                  className="text-[var(--t5)] hover:text-[var(--danger,#ef4444)]"
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="mt-3 flex items-center justify-between gap-2">
          <button
            className="nm-outset flex items-center gap-1 rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
            onClick={onAddAttachments}
            disabled={decomposing}
            title="添加参考文件（docx/pdf/txt 等）：AI 拆解前先读一遍，内容会拆进各任务卡的备注或子任务"
          >
            📎 添加附件
          </button>
          {decomposing ? (
            <span className="flex items-center gap-1.5 text-xs text-[var(--t5)]">
              <Loader2 size={13} className="animate-spin" aria-hidden /> AI 拆解中…
            </span>
          ) : (
            <button
              className="nm-inset rounded-[var(--r-sm)] px-4 py-1.5 text-sm text-[var(--t1)] disabled:cursor-not-allowed disabled:opacity-50"
              disabled={!goal.trim()}
              title={goal.trim() ? "AI 拆解为任务卡并自动连线" : "先填写目标描述"}
              onClick={onDecompose}
            >
              {isRegenerate ? "AI 重新生成" : "AI 生成"}
            </button>
          )}
          {decomposing && (
            <button
              className="nm-outset rounded-[var(--r-sm)] px-4 py-1.5 text-sm text-[var(--t3)]"
              onClick={onCancelDecompose}
            >
              取消
            </button>
          )}
        </div>
        {workflows.length > 0 && (
          <div className="mt-5 border-t border-[var(--edge)] pt-4">
            <p className="text-xs font-medium text-[var(--t5)]">已有工作流</p>
            <div className="mt-2 flex flex-wrap gap-2">
              {workflows.map((w) => (
                <button
                  key={w.id}
                  className="nm-outset rounded-[var(--r-sm)] px-3 py-1 text-sm text-[var(--t2)]"
                  onClick={() => onOpen(w.id)}
                >
                  {w.name}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
