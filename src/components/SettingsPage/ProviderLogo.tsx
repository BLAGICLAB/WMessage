// SettingsPage 子模块：厂商 logo 展示件。
// 解析链：resolveLobeIcon（lobehub 本地资产，providerKey 优先、name 次之）→
// 未命中回退圆形色块 + 首字母（fallbackColor/fallbackChar，缺省 #666666 / 名称首字）。
// 本地资产无加载失败分支，不再消费 models.dev 在线 logo。
// 用在三处：厂商详情页头、左栏厂商列表项、添加厂商预设/模型库网格。

import { resolveLobeIcon } from "./providerLogoMap";

export function ProviderLogo({
  name,
  providerKey,
  className = "h-5 w-5",
  fallbackColor,
  fallbackChar,
}: {
  name: string;
  /** models.dev provider_key；传入即优先于 name 解析图标 */
  providerKey?: string | null;
  className?: string;
  /** 未命中图标时的圆形色块底色 */
  fallbackColor?: string;
  /** 未命中图标时的色块字符（缺省取厂商名首字） */
  fallbackChar?: string;
}) {
  const src = resolveLobeIcon(providerKey ?? undefined, name);
  if (src) {
    // alt 留空：logo 恒与厂商名文本相邻（装饰性），避免可访问名重复朗读
    return <img src={src} alt="" className={`shrink-0 rounded ${className}`} />;
  }
  return (
    <span
      aria-hidden
      className={`shrink-0 inline-flex items-center justify-center rounded-full text-white ${className}`}
      style={{ background: fallbackColor ?? "#666666", fontSize: "10px", lineHeight: 1 }}
    >
      {/* || 而非 ??：调用方可能传空串（数据缺 fallback_char），空串同样回退首字 */}
      {fallbackChar || name.slice(0, 1) || "?"}
    </span>
  );
}
