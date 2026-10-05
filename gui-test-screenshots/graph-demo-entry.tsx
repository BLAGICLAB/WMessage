// 任务图谱浏览器冒烟入口（gui 冒烟专用，不入构建）：真实 GraphPage 组件 +
// seed.sql 同源数据，Tauri 面 mock（见 graph-demo.html）。用后即删。
import { createRoot } from "react-dom/client";
import { createElement } from "react";
import GraphPage from "../src/components/GraphPage/GraphPage";
import "../src/ui/main.css";

const t = (
  id: string,
  title: string,
  column: "todo" | "doing" | "done",
  extra: Record<string, unknown> = {}
) => ({ id, title, column, ...extra });

const now = Date.now();
const DAY = 86_400_000;
const d = (daysAgo: number) => now - daysAgo * DAY;

const tasks = [
  // 本人的个人卡（createdAt = 创建时间，耗时模式的视觉验证数据）
  t("m0", "梳理季度 OKR", "todo", { tags: ["管理"], due: "2026-10-10", createdAt: d(2) }),
  t("m1", "写十月第一周周报", "doing", { tags: ["周报"], due: "2026-10-11", createdAt: d(4) }),
  t("m2", "评审新人方案", "done", { tags: ["评审"], completedAt: d(5), createdAt: d(12) }),
  t("m3", "整理会议纪要归档", "done", { tags: ["纪要"], completedAt: d(20), archived: true, createdAt: d(55) }),
  t("m4", "准备部门分享 PPT", "todo", { createdAt: d(1) }),
  t("m5", "对接飞书机器人 API", "doing", { tags: ["飞书"], createdAt: d(16) }),
  t("m6", "修复看板拖拽 bug", "done", { tags: ["bug"], completedAt: d(12), archived: true, createdAt: d(13) }),
  // 本人的工作流 1：周报流水线（4 卡 DAG）
  t("w1a", "收集本周任务动态", "done", { origin: "workflow", workflowId: "wf-demo-1", tags: ["周报"], completedAt: d(9), archived: true }),
  t("w1b", "汇总飞书群消息要点", "done", { origin: "workflow", workflowId: "wf-demo-1", dependsOn: ["w1a"], tags: ["周报"], completedAt: d(9), archived: true }),
  t("w1c", "起草周报初稿", "done", { origin: "workflow", workflowId: "wf-demo-1", dependsOn: ["w1a", "w1b"], tags: ["周报"], completedAt: d(9), archived: true }),
  t("w1d", "终审并发出周报", "doing", { origin: "workflow", workflowId: "wf-demo-1", dependsOn: ["w1c"], tags: ["周报"], note: "产出：部门周报邮件" }),
  // 本人的工作流 2：官网部署（3 卡）
  t("w2a", "构建前端产物", "done", { origin: "workflow", workflowId: "wf-demo-2", completedAt: d(2) }),
  t("w2b", "上传到服务器", "done", { origin: "workflow", workflowId: "wf-demo-2", dependsOn: ["w2a"], completedAt: d(2) }),
  t("w2c", "线上冒烟验证", "todo", { origin: "workflow", workflowId: "wf-demo-2", dependsOn: ["w2b"] }),
  // 张三
  t("z0", "整理客户反馈清单", "todo", { ownerId: "p-zhang-demo", tags: ["客户"], createdAt: d(6) }),
  t("z1", "给大客户写方案", "doing", { ownerId: "p-zhang-demo", tags: ["方案"], createdAt: d(21) }),
  t("z2", "月度销售数据汇总", "done", { ownerId: "p-zhang-demo", tags: ["数据"], completedAt: d(3), createdAt: d(33) }),
  t("z3", "回访 A 客户", "done", { ownerId: "p-zhang-demo", tags: ["客户"], completedAt: d(8), createdAt: d(9) }),
  t("zw1", "抓取竞品价格", "done", { ownerId: "p-zhang-demo", completedAt: d(5), createdAt: d(7) }),
  t("zw2", "生成对比表", "done", { ownerId: "p-zhang-demo", dependsOn: ["zw1"], completedAt: d(5), createdAt: d(6) }),
  t("zw3", "写入竞品简报", "todo", { ownerId: "p-zhang-demo", dependsOn: ["zw2"], createdAt: d(2) }),
  // 李四
  t("l0", "服务器巡检", "done", { ownerId: "p-li-demo", tags: ["运维"], completedAt: d(30), archived: true, createdAt: d(31) }),
  t("l1", "数据库备份演练", "done", { ownerId: "p-li-demo", tags: ["运维"], completedAt: d(15), archived: true, createdAt: d(60) }),
  t("l2", "整理机房资产表", "todo", { ownerId: "p-li-demo", createdAt: d(3) }),
  t("l3", "升级内网网关", "doing", { ownerId: "p-li-demo", tags: ["运维"], createdAt: d(10) }),
  t("l4", "编写运维手册", "todo", { ownerId: "p-li-demo", tags: ["文档"], createdAt: d(1) }),
  // 2025 年（年份过滤器演示）：无 createdAt = 老数据未知耗时 → 最小尺寸
  t("old1", "2025 年度总结", "done", { completedAt: 1749945600000, archived: true }),
];

const root = createRoot(document.getElementById("root")!);

// ?stress=N：生成 N 个合成任务（含 hub/依赖链/多人），压测过滤点击与绘制
function stressTasks(n: number): Task[] {
  const out: Task[] = [];
  const owners = [undefined, "p-zhang-demo", "p-li-demo"];
  for (let i = 0; i < n; i++) {
    const col = (["todo", "doing", "done"] as const)[i % 3];
    out.push({
      id: `s${i}`,
      title: `压测任务-${i}`,
      column: col,
      ownerId: owners[i % 3],
      tags: i % 7 === 0 ? ["压测"] : undefined,
      origin: i % 5 === 0 ? "workflow" : undefined,
      workflowId: i % 5 === 0 ? `wf-s${i % 40}` : undefined,
      dependsOn: i > 4 && i % 3 === 0 ? [`s${i - 3}`, `s${i - 4}`] : undefined,
      completedAt: col === "done" ? Date.now() - (i % 300) * 86_400_000 : undefined,
    });
  }
  return out;
}

const stressParam = new URLSearchParams(location.search).get("stress");
const phase = (p: string) => {
  window.__phase = p;
  document.title = `[${p}] 图谱压测`;
};
phase("creating-tasks");
const demoTasks = stressParam ? [...tasks, ...stressTasks(Number(stressParam))] : tasks;
phase("rendering");
root.render(
  createElement("div", { style: { height: "100vh", width: "100vw" } },
    createElement(GraphPage, {
      tasks: demoTasks,
      onOpenTask: () => {},
      onOpenWorkflow: () => {},
    })
  )
);
phase("render-done");
