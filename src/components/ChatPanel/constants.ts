// ChatPanel 子模块：常量集合（模块级变量 + 斜杠命令清单）。
// 不依赖 React；纯数据 / 跨实例共享状态。

import type {
  ModelItem,
  PermMode,
  ReasoningLevel,
  SubagentRow,
  VerboseLevel,
} from "./types";

/** execute-task 事件去重窗口（毫秒）：同一 id 窗口内重复触发直接跳过 */
const EXEC_TASK_DEDUP_MS = 2000;

/** execute-task 事件去重（模块级，跨组件实例/HMR 泄漏监听器共享）：
 *  dev 期间组件多次重挂载会累积多个 execute-task 监听器，
 *  一次点击被投递多次 → 同一秒多个 bot_execute_task 并发 → 后端防重入拦截，
 *  每个拒绝都弹「⚠️ 内部错误：该任务卡正在执行中」气泡，用户误以为执行失败。
 *  去重表必须放模块级：放 useEffect 闭包里则每个泄漏监听器各持一份，去重失效。
 *  表本体不导出——只暴露 shouldSkip 单入口，调用顺带 prune 超窗条目防无界增长 */
const dedupTable = new Map<string, number>();
/** 任务执行收尾表（chat-open-session 迟到防复活）：taskId → 执行结果。 * 进程内存活不清理（任务 id 是 UUID 无碰撞，量级百字节级）；：失败任务
 * 的迟到跳转不自动切，停下来让用户决定是否重试/查看 */
export const finishedExecTasks = new Map<string, "success" | "failed">();

export const execTaskDedup = {
  /** 同一 id 在 EXEC_TASK_DEDUP_MS 窗口内重复触发 → true（调用方应 return） */
  shouldSkip(id: string, now: number): boolean {
    for (const [k, t] of dedupTable) {
      if (now - t >= EXEC_TASK_DEDUP_MS) dedupTable.delete(k);
    }
    const last = dedupTable.get(id);
    if (last !== undefined && now - last < EXEC_TASK_DEDUP_MS) return true;
    dedupTable.set(id, now);
    return false;
  },
};
/** 流式增量合并窗口（约一帧）：同一窗口内到达的 SSE 片段攒起来一次写 state */
export const DELTA_BATCH_MS = 16;

/** 气泡列宽上限（占阅读带宽度百分比）：用户与机器人**同一条**上限。
 *  机器人气泡原先是满宽，长回复一路伸到用户头像底下，两边不等宽——
 *  一条上限两侧共用，机器人气泡不再侵入头像那一列。
 *  粘性下限（stickyBubbleWidth）按同一个百分比封顶，两处共用同一份真相。 */
export const BUBBLE_MAX_W_PCT = 85;

/** 在途子 agent 的行重拉周期（毫秒）。轮数只活在后端内存仪表里，不轮询就
 *  永远看不到推进；3 秒够——子 agent 一轮最短也要几秒（要调模型）。 */
export const SUBAGENT_POLL_MS = 3000;

/** 斜杠命令清单（单一真相：autocomplete picker + runSlashCommand 共享）。
 *  没有 /help：上浮全面板后 /help 还在 LLM 上下文里白白占 token */
export const SLASH_COMMANDS = [
  { cmd: "/stop", description: "停止当前回复" },
  { cmd: "/compact", description: "压缩对话上下文" },
  { cmd: "/clean", description: "清空当前对话" },
  { cmd: "/retry", description: "重新生成上一条回复" },
];

/** 推理强度档位展示文案（RE-1） */
export const EFFORT_LABELS: Record<ReasoningLevel, string> = {
  off: "关闭",
  low: "低",
  medium: "中",
  high: "高",
};

/** 模型协议分组展示文案（双协议同列） */
export const PROVIDER_LABELS: Record<ModelItem["provider"], string> = {
  openai: "OpenAI 兼容",
  anthropic: "Anthropic 兼容",
};

/** 授权模式短标签：进 pill 与下拉行首 */
export const PERM_LABELS: Record<PermMode, string> = {
  ask: "弹授权",
  strict: "硬拒",
  auto: "自动",
  yolo: "全放行",
};

/** 授权模式一行完整语义（下拉右侧说明 + pill 的 title 共用一处）。
 *  「自动」档的两面必须写全：白名单内自动放行、白名单外降级弹窗——
 *  只写前半句会让人以为它就是全放行。 */
export const PERM_HINTS: Record<PermMode, string> = {
  ask: "白名单外弹授权窗",
  strict: "白名单外直接拒绝，不弹窗",
  auto: "白名单内自动放行，白名单外仍弹窗",
  yolo: "全部放行不弹窗（仍记审计）",
};

/** 下拉档位顺序：由严到宽。用户在这排里挑的是「放行多宽」，
 *  按宽度排比按枚举顺序更好比较——这是安全档，不做记忆化排序。 */
export const PERM_ORDER: readonly PermMode[] = ["strict", "ask", "auto", "yolo"] as const;

/** 执行过程详细度（ verbose 三档）：短标签进 pill，语义见 types.ts */
export const VERBOSE_LABELS: Record<VerboseLevel, string> = {
  concise: "简洁",
  detailed: "详细",
  debug: "调试",
};

/** 子 agent 六态 → 中文标签 + 徽标色（右侧栏徽标与收尾提示共用一份）。
 *  取消/预算耗尽各有自己的说法：都写成「失败」等于把用户的选择说成系统出错 */
export const SUBAGENT_STATUS: Record<
  SubagentRow["status"],
  { label: string; color: string }
> = {
  queued: { label: "排队中", color: "text-[var(--t5)]" },
  running: { label: "执行中", color: "text-[var(--brand)]" },
  succeeded: { label: "完成", color: "text-[var(--success)]" },
  failed: { label: "失败", color: "text-[var(--danger)]" },
  cancelled: { label: "已取消", color: "text-[var(--t5)]" },
  budget_exceeded: { label: "预算耗尽", color: "text-[var(--danger)]" },
};
