/** 大模型提供商预设（老板拍板清空）：
 *  设置页已改成"双协议下独立大模型列表"，用户自己点「添加大模型」维护；
 *  硬编码的供应商预设（MiniMax/Kimi/DeepSeek）已不再需要——任何"加默认厂商"
 *  都会和"不设置默认厂商"的产品决策冲突。
 *
 *  本文件保留仅为向后兼容：ChatPanel 头部标签 + 切换菜单还在 import，
 *  列表为空时菜单自然不渲染，标签走 fallback "未配置"。
 *  新代码不要再往 PROVIDER_PRESETS 里塞供应商。 */
export type ProviderPreset = {
  label: string;
  baseUrl: string;
  model: string;
};

export const PROVIDER_PRESETS: ProviderPreset[] = [];

/** 当前配置命中的预设（baseUrl + model 双匹配——ChatPanel 头部标签显示用）；
 *  列表为空时永远返回 undefined，ChatPanel 拿到后走 fallback "未配置"。 */
export function matchPreset(_baseUrl: string, _model: string): ProviderPreset | undefined {
  return undefined;
}
