/** 大模型提供商预设：设置页 + 聊天窗口模型切换共用。
 *  点击自动填 Base URL + 推荐模型，仍可手动修改。
 *  2026-08-20 更新：Kimi K2 下线换 K3（kimi-k3）；DeepSeek 旧名 deepseek-chat 已废弃，
 *  换 V4 双档（deepseek-v4-flash / deepseek-v4-pro，同 baseUrl，靠 model 区分）。 */
export const PROVIDER_PRESETS = [
  { label: "MiniMax", baseUrl: "https://api.minimaxi.com/v1", model: "MiniMax-M3" },
  { label: "Kimi K3", baseUrl: "https://api.moonshot.cn/v1", model: "kimi-k3" },
  { label: "DeepSeek V4 Flash", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-v4-flash" },
  { label: "DeepSeek V4 Pro", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-v4-pro" },
] as const;

export type ProviderPreset = (typeof PROVIDER_PRESETS)[number];

/** 当前配置命中的预设（baseUrl + model 双匹配——DeepSeek 两档同 baseUrl，单靠地址无法区分）；
 *  自定义地址/模型返回 undefined */
export function matchPreset(baseUrl: string, model: string): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find(
    (p) => p.baseUrl === baseUrl.trim() && p.model === model.trim()
  );
}
