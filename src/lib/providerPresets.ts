/** 大模型提供商预设：设置页 + 聊天窗口模型切换共用。
 *  点击自动填 Base URL + 默认模型，模型可在下方「模型」栏手动改（如 deepseek-v4-pro）。
 *  2026-09-05 改版（老板拍板）：预设收敛为纯供应商维度（MiniMax/Kimi/DeepSeek），
 *  不再按模型分档（原 DeepSeek V4 Flash/Pro 双按钮合并），模型型号在模型栏填写。 */
export const PROVIDER_PRESETS = [
  { label: "MiniMax", baseUrl: "https://api.minimaxi.com/v1", model: "MiniMax-M3" },
  { label: "Kimi", baseUrl: "https://api.moonshot.cn/v1", model: "kimi-k3" },
  { label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-v4-flash" },
] as const;

export type ProviderPreset = (typeof PROVIDER_PRESETS)[number];

/** 当前配置命中的预设（baseUrl + model 双匹配——挂件菜单标签显示用；
 *  自定义地址/模型返回 undefined） */
export function matchPreset(baseUrl: string, model: string): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find(
    (p) => p.baseUrl === baseUrl.trim() && p.model === model.trim()
  );
}

/** 按供应商匹配（只看 baseUrl，不看 model——2026-09-05 设置页预设高亮用：
 *  模型栏手改成同供应商其它型号时，供应商按钮保持高亮） */
export function matchProvider(baseUrl: string): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find((p) => p.baseUrl === baseUrl.trim());
}
