// SettingsPage 子模块：常量数组 + 派生的联合类型字面量。
// 由 ApiProviderSelect 与 main SettingsPage 共享。

export const API_PROVIDER_OPTIONS = [
  { value: "openai", label: "OpenAI 兼容" },
  { value: "anthropic", label: "Anthropic 兼容" },
] as const;
export type ApiProvider = (typeof API_PROVIDER_OPTIONS)[number]["value"];

/** 界面字体大小四档：顺序 = 从小到大，
 *  索引位置 = SettingsPage 滑块/按钮的档位 */
export const UI_FONT_SIZE_OPTIONS = [
  { value: "small", label: "小" },
  { value: "standard", label: "标准" },
  { value: "large", label: "大" },
  { value: "xlarge", label: "特大" },
] as const;
export type UiFontSize = (typeof UI_FONT_SIZE_OPTIONS)[number]["value"];
