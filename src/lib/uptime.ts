// 运行时长格式化：秒 → 人读时长。
//
// 两个面板都要它（受管服务面板 / MCP 面板的资源行），所以放共享 lib
// 而不是各自复制一份——两份各改各的，展示会漂。

/** 秒 → 人读时长：秒 / 分 / 小时三档。 */
export function formatUptime(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分 ${s % 60} 秒`;
  const h = Math.floor(s / 3600);
  return `${h} 小时 ${Math.floor((s % 3600) / 60)} 分`;
}
