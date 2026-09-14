// ChatPanel 子模块：常量集合（模块级变量 + 斜杠命令清单）。
// 不依赖 React；纯数据 / 跨实例共享状态。

/** execute-task 事件去重（模块级，跨组件实例/HMR 泄漏监听器共享）：
 *  dev 期间挂件 webview 多次重挂载会累积多个 execute-task 监听器，
 *  一次点击被投递多次 → 同一秒多个 bot_execute_task 并发 → 后端防重入拦截，
 *  每个拒绝都弹「⚠️ 内部错误：该任务卡正在执行中」气泡，用户误以为执行失败。
 *  去重表必须放模块级：放 useEffect 闭包里则每个泄漏监听器各持一份，去重失效。 */
export const execTaskDedup = new Map<string, number>();
export const EXEC_TASK_DEDUP_MS = 2000;
/** 流式增量合并窗口（约一帧）：同一窗口内到达的 SSE 片段攒起来一次写 state */
export const DELTA_BATCH_MS = 16;

/** 斜杠命令清单（单一真相：autocomplete picker + runSlashCommand 共享）。
 *  没有 /help：上浮全面板后 /help 还在 LLM 上下文里白白占 token */
export const SLASH_COMMANDS = [
  { cmd: "/stop", description: "停止当前回复" },
  { cmd: "/compact", description: "压缩对话上下文" },
  { cmd: "/clean", description: "清空当前对话" },
  { cmd: "/retry", description: "重新生成上一条回复" },
];
