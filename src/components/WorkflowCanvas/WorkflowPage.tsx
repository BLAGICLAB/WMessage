import { useCallback, useEffect, useMemo, useState } from "react";
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
import { Download, Play, Plus, RefreshCw, Save, Trash2, Upload } from "lucide-react";
import { handleCommandError } from "../../lib/errorHandler";
import type { Task, Workflow, WorkflowSaveResult } from "../../types";
import {
  draftFromTasks,
  wouldCreateCycle,
  type CanvasNode,
} from "./graph";
import { TaskNode, type TaskNodeData } from "./TaskNode";
import { GoalNode, type GoalNodeData } from "./GoalNode";

/** 总目标卡在画布上的固定节点 id（绑定 workflows 元数据，非任务卡） */
const GOAL_ID = "__goal__";

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

  // 工作流列表（进入页面拉一次）
  useEffect(() => {
    invoke<Workflow[]>("workflow_list")
      .then(setWorkflows)
      .catch((e) => handleCommandError(e, "读取工作流列表", { silent: true }));
  }, []);

  const snapshot = useCallback(
    () => JSON.stringify({ name, goal, nodes }),
    [name, goal, nodes]
  );
  const dirty = savedSnapshot !== snapshot();

  // ────────────── 打开/新建 ──────────────

  const openWorkflow = async (id: string) => {
    try {
      const detail = await invoke<Workflow & { tasks: Task[] }>("workflow_load", { id });
      setActiveId(id);
      setName(detail.name);
      setGoal(detail.goal);
      setNodes(draftFromTasks(detail.tasks));
      setSelectedIds([]);
      setSavedSnapshot(
        JSON.stringify({
          name: detail.name,
          goal: detail.goal,
          nodes: draftFromTasks(detail.tasks),
        })
      );
      setMode("edit");
      setDeleteArmed(false);
    } catch (e) {
      handleCommandError(e, "打开工作流", { onRetry: () => void openWorkflow(id) });
    }
  };

  const createBlank = () => {
    setActiveId(null);
    setName(`工作流 ${new Date().toLocaleDateString()}`);
    setGoal("");
    setNodes([]);
    setSelectedIds([]);
    setSavedSnapshot(null);
    setMode("edit");
    setDeleteArmed(false);
  };

  // ────────────── 节点编辑 ──────────────

  const addNode = () => {
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

  const deleteNode = (localId: string) => {
    // 连带清理：删掉它自己 + 所有人对它的依赖（设计 §决策8）
    setNodes((prev) =>
      prev
        .filter((n) => n.localId !== localId)
        .map((n) => ({ ...n, dependsOn: n.dependsOn.filter((d) => d !== localId) }))
    );
    setSelectedIds((prev) => prev.filter((s) => s !== localId));
  };

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
          // edge id = `${source}->${target}`；删边 = 下游 dependsOn 移除上游
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

  const commitTitle = (taskId: string, title: string) => {
    setNodes((prev) =>
      prev.map((n) => (n.taskId === taskId ? { ...n, title } : n))
    );
    onUpdate(taskId, { title });
  };

  const toggleDone = (taskId: string) => {
    const t = tasks.find((x) => x.id === taskId);
    if (!t) return;
    onSetColumn(taskId, t.column === "done" ? "todo" : "done");
  };

  const toggleSubtask = (taskId: string, subtaskId: string) => {
    const t = tasks.find((x) => x.id === taskId);
    if (!t?.subtasks) return;
    onUpdate(taskId, {
      subtasks: t.subtasks.map((s) =>
        s.id === subtaskId ? { ...s, done: !s.done } : s
      ),
    });
  };

  // ────────────── 保存（指纹 diff 落库，设计 §7） ──────────────

  const save = async () => {
    if (saving) return;
    setSaving(true);
    try {
      const res = await invoke<WorkflowSaveResult>("workflow_save", {
        input: {
          workflowId: activeId,
          name: name.trim() || "未命名工作流",
          goal: goal.trim() || name.trim() || "未命名工作流",
          nodes: nodes.map((n) => ({
            localId: n.localId,
            taskId: n.taskId ?? null,
            title: n.title,
            note: n.note ?? null,
            tags: n.tags ?? null,
            dependsOn: n.dependsOn,
            pos: n.pos,
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
      setSavedSnapshot(JSON.stringify({ name, goal, nodes: next }));
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
      setTimeout(() => setDeleteArmed(false), 3000);
      return;
    }
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

  const rfNodes = useMemo<Node<TaskNodeData | GoalNodeData>[]>(
    () => [
      {
        id: GOAL_ID,
        type: "goal",
        position: { x: 40, y: 40 },
        draggable: false,
        selectable: false,
        deletable: false,
        data: { name, goal, saved: savedSnapshot !== null, onRename: setName, onGoalChange: setGoal },
      },
      ...nodes.map((n) => {
        const task = n.taskId ? tasks.find((t) => t.id === n.taskId) : undefined;
        const data: TaskNodeData = {
          task,
          draftTitle: n.title,
          onDraftTitleCommit: (localId, title) =>
            setNodes((prev) =>
              prev.map((m) => (m.localId === localId ? { ...m, title } : m))
            ),
          onDelete: deleteNode,
          onToggleDone: task ? toggleDone : undefined,
          onCommitTitle: task ? commitTitle : undefined,
          onToggleSubtask: task ? toggleSubtask : undefined,
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [nodes, tasks, name, goal, savedSnapshot, selectedIds]
  );

  const rfEdges = useMemo<Edge[]>(
    () =>
      nodes.flatMap((n) =>
        n.dependsOn
          .filter((d) => nodes.some((m) => m.localId === d))
          .map((d) => ({
            id: `${d}->${n.localId}`,
            source: d,
            target: n.localId,
          }))
      ),
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
          className={toolbarBtn}
          disabled
          title="AI 拆解生成将在 W2 批次上线"
        >
          <RefreshCw size={14} aria-hidden /> 重新生成
        </button>
        <button className={toolbarBtn} disabled title="模板导入导出将在 W4 批次上线">
          <Upload size={14} aria-hidden /> 导入
        </button>
        <button className={toolbarBtn} disabled title="模板导入导出将在 W4 批次上线">
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
        <button
          className={toolbarBtn}
          disabled
          title="执行引擎将在 W3 批次上线（先保存，后执行）"
        >
          <Play size={14} aria-hidden /> 开始执行
        </button>
        {activeId && (
          <button
            className={`${toolbarBtn} ${deleteArmed ? "nm-inset text-[var(--danger,#ef4444)]" : ""}`}
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
            onCreateBlank={createBlank}
            workflows={workflows}
            onOpen={(id) => void openWorkflow(id)}
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

/** 空态引导（设计 §5.1 空态）：一句话目标输入 + 创建空白 + 已有工作流列表 */
function EmptyHero({
  goal,
  onGoalChange,
  onCreateBlank,
  workflows,
  onOpen,
}: {
  goal: string;
  onGoalChange: (v: string) => void;
  onCreateBlank: () => void;
  workflows: Workflow[];
  onOpen: (id: string) => void;
}) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="nm-card w-full max-w-xl p-6">
        <h2 className="text-base font-semibold text-[var(--t1)]">新建工作流</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          描述你想让 AI 帮你完成的工作流程，AI 将拆解成任务卡并自动连线（AI 拆解 W2 上线），也可以先创建空白画布手动编排。
        </p>
        <textarea
          aria-label="工作流目标描述"
          className="mt-4 h-24 w-full resize-none rounded-[var(--r-sm)] nm-inset p-3 text-sm text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
          placeholder="例：每周五收集本周完成的任务，汇总成一份周报文档并绑定到任务卡"
          value={goal}
          onChange={(e) => onGoalChange(e.target.value)}
        />
        <div className="mt-3 flex justify-end gap-2">
          <button
            className="nm-outset rounded-[var(--r-sm)] px-4 py-1.5 text-sm text-[var(--t3)] opacity-50"
            disabled
            title="AI 拆解生成将在 W2 批次上线"
          >
            AI 生成
          </button>
          <button
            className="nm-inset rounded-[var(--r-sm)] px-4 py-1.5 text-sm text-[var(--t1)]"
            onClick={onCreateBlank}
          >
            创建空白工作流
          </button>
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
