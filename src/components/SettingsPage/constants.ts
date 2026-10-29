// SettingsPage 子模块：常量数组 + 派生的联合类型字面量。
// 由 ApiProviderSelect 与 main SettingsPage 共享。

import type { ProviderPreset } from "./types";

export const API_PROVIDER_OPTIONS = [
  { value: "openai", label: "OpenAI 兼容" },
  { value: "anthropic", label: "Anthropic 兼容" },
] as const;
export type ApiProvider = (typeof API_PROVIDER_OPTIONS)[number]["value"];

/** 界面字体大小三档：顺序 = 从小到大，
 *  索引位置 = SettingsPage 滑块/按钮的档位 */
export const UI_FONT_SIZE_OPTIONS = [
  { value: "small", label: "小" },
  { value: "standard", label: "标准" },
  { value: "large", label: "大" },
] as const;
export type UiFontSize = (typeof UI_FONT_SIZE_OPTIONS)[number]["value"];


export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    name: "DeepSeek",
    protocol: "openai",
    baseUrl: "https://api.deepseek.com",
    keyUrl: "https://platform.deepseek.com/api_keys",
    models: [
      { id: "deepseek-chat", contextK: 128 },
      { id: "deepseek-reasoner", contextK: 128 },
    ],
  },
  {
    name: "Kimi",
    protocol: "openai",
    baseUrl: "https://api.moonshot.cn/v1",
    keyUrl: "https://platform.moonshot.cn/console/api-keys",
    models: [
      { id: "kimi-k2-0711-preview", contextK: 256 },
      { id: "moonshot-v1-8k", contextK: 8 },
    ],
  },
  {
    name: "MiniMax",
    protocol: "anthropic",
    baseUrl: "https://api.minimaxi.com/anthropic",
    keyUrl:
      "https://platform.minimaxi.com/user-center/basic-information/interface-key",
    models: [
      { id: "MiniMax-M3", contextK: 1000 },
      { id: "MiniMax-M2", contextK: 204.8 },
    ],
  },
  {
    name: "OpenRouter",
    protocol: "openai",
    baseUrl: "https://openrouter.ai/api/v1",
    keyUrl: "https://openrouter.ai/keys",
    models: [{ id: "openrouter/auto" }],
  },
  {
    name: "阿里云百炼",
    protocol: "openai",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    keyUrl: "https://bailian.console.aliyun.com/?apiKey=1#/api-key",
    models: [
      { id: "qwen-plus", contextK: 1024 },
      { id: "qwen-max", contextK: 32 },
    ],
  },
  {
    name: "OpenAI",
    protocol: "openai",
    baseUrl: "https://api.openai.com/v1",
    keyUrl: "https://platform.openai.com/api-keys",
    models: [
      { id: "gpt-4o", contextK: 128 },
      { id: "gpt-4o-mini", contextK: 128 },
    ],
  },
  {
    name: "Anthropic",
    protocol: "anthropic",
    baseUrl: "https://api.anthropic.com",
    keyUrl: "https://console.anthropic.com/settings/keys",
    models: [
      { id: "claude-sonnet-4-20250514", contextK: 200 },
    ],
  },
  {
    name: "xAI",
    protocol: "openai",
    baseUrl: "https://api.x.ai/v1",
    keyUrl: "https://console.x.ai/",
    models: [{ id: "grok-4", contextK: 256 }],
  },
];
/** 厂商详情页 API 格式下拉选项（ApiProviderSelect 默认是协议短名，这里用全名） */
export const API_FORMAT_OPTIONS = [
  { value: "openai", label: "OpenAI Chat Completions" },
  { value: "anthropic", label: "Anthropic Messages" },
] as const;
/** 自动记忆抽取三档（档位唯一事实源：state 默认/校验/UI 映射都从这里派生） */
export const AUTO_EXTRACT_MODES = [
  { value: "off", label: "关闭", aria: "自动记忆抽取：关闭" },
  { value: "auto", label: "自动入库", aria: "自动记忆抽取：自动入库" },
  { value: "confirm", label: "需确认", aria: "自动记忆抽取：需确认" },
] as const;
export type AutoExtractMode = (typeof AUTO_EXTRACT_MODES)[number]["value"];
export const isAutoExtractMode = (v: unknown): v is AutoExtractMode =>
  AUTO_EXTRACT_MODES.some((m) => m.value === v);
/** 已知厂商的双协议 Base URL（key = 归一化厂商名，见 normalizeVendorName）：
 *  API 格式切换时 Base URL 自动跟随的数据源之一（预设/模型库只带单一协议端点，
 *  这里补「另一协议」的已知端点——目前只有 MiniMax 双协议都有官方端点） */
export const KNOWN_PROTOCOL_URLS: Record<
  string,
  { openai?: string; anthropic?: string }
> = {
  minimax: {
    openai: "https://api.minimaxi.com/v1",
    anthropic: "https://api.minimaxi.com/anthropic",
  },
};
/** 添加厂商网格白名单（models.dev 200+ 家收敛）：中国厂商尽量保留，
 *  外国厂商只留头部（OpenAI/Anthropic/Google/xAI）+ OpenRouter */
export const CURATED_PROVIDER_KEYS = new Set([
  // 中国厂商
  "302ai",
  "aihubmix",
  "alibaba",
  "alibaba-cn",
  "alibaba-coding-plan",
  "alibaba-coding-plan-cn",
  "alibaba-token-plan",
  "alibaba-token-plan-cn",
  "bailing",
  "deepseek",
  "iflowcn",
  "kimi-code-plan-cn",
  "longcat",
  "minimax",
  "minimax-cn",
  "minimax-cn-coding-plan",
  "minimax-coding-plan",
  "modelscope",
  "moonshotai",
  "moonshotai-cn",
  "qiniu-ai",
  "sensenova",
  "siliconflow",
  "siliconflow-cn",
  "stepfun",
  "stepfun-ai",
  "stepfun-ai-step-plan",
  "stepfun-step-plan",
  "tencent-coding-plan",
  "tencent-token-plan",
  "tencent-tokenhub",
  "volcengine",
  "volcengine-coding-plan",
  "xiaomi",
  "xiaomi-token-plan-cn",
  "zai",
  "zai-coding-plan",
  "zhipuai",
  "zhipuai-coding-plan",
  // 头部外国厂商 + OpenRouter
  "openai",
  "anthropic",
  "google",
  "xai",
  "openrouter",
]);
/** Agent 运行参数卡的行定义：后端参数表 key + 对应的 config 字段名。
 *
 *  这里只声明「哪一行、编辑哪个字段」——标签/说明/默认值/可填范围/当前生效值
 *  全部来自 `bot_effective_params`（后端 `bot::params` 单源）。前端此前自留了
 *  一份默认数字，两份各改各的：后端改了默认、页面还写着旧数，页面上「默认 X」
 *  与实际生效的 Y 就是两回事。
 */
export const AGENT_PARAM_ROWS = [
  ["loop.max_rounds", "maxRounds"],
  ["loop.max_function_calls", "maxFunctionCalls"],
  ["loop.history_budget_chars", "historyBudgetChars"],
  ["loop.history_truncate_target_pct", "historyTruncateTargetPct"],
  ["loop.reasoning_echo_enabled", "reasoningEchoEnabled"],
  ["loop.in_turn_context_edit", "inTurnContextEdit"],
  ["search.max_results", "searchMaxResults"],
  ["tools.max_output_bytes", "maxToolOutputBytes"],
  ["tools.ask_budget", "askBudget"],
] as const;
/** 降级用的行名（后端参数表拉不到时用）。
 *  只放**名字**、不放任何数字——默认值与范围一律来自后端，前端多抄一份数字
 *  就是多一个能各自漂移的真相源。 */
export const AGENT_PARAM_FALLBACK_LABELS: Record<string, string> = {
  "loop.max_rounds": "模型循环最大轮数",
  "loop.max_function_calls": "调用工具上限（全域熔断）",
  "loop.history_budget_chars": "会话历史字符预算",
  "loop.history_truncate_target_pct": "历史截断滞回比例（%）",
  "loop.reasoning_echo_enabled": "思考段回灌",
  "loop.in_turn_context_edit": "轮内工具结果滑动窗",
  "search.max_results": "联网搜索默认条数",
  "tools.max_output_bytes": "工具结果截断字节数",
  "tools.ask_budget": "任务提问预算（次）",
};
/** u32 上界——config 里这些字段是 Option<u32>，超了整份 bot_set_config
 *  反序列化失败（一处输错字连带整份设置都存不下去）。 */
export const U32_MAX = 4294967295;
