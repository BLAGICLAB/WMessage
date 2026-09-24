// ChatPanel 子模块：流式文本渲染（http(s) 链接 + 绝对文件路径可点击）。
// 链接/路径识别规则在 lib/openTarget（带空格路径不会截断成「C:\Program」）。

import type { ReactNode } from "react";
import {
  LINK_OR_PATH_RE,
  isHttpUrl,
  openTarget,
} from "../../lib/openTarget";

export function RichText({ text }: { text: string }) {
  const parts: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(LINK_OR_PATH_RE)) {
    const idx = m.index ?? 0;
    const token = m[0];
    if (idx > last) parts.push(<span key={key++}>{text.slice(last, idx)}</span>);
    const isUrl = isHttpUrl(token);
    const clean = isUrl ? token.replace(/[.,;:!?]+$/, "") : token;
    parts.push(
      // URL 给真 href；路径不设 href（防中键/复制链接把文件路径当 URL），
      // 补 role/tabIndex/Enter 保持键盘可达（与 MarkdownText 行内代码链接同形态）
      <a
        key={key++}
        href={isUrl ? clean : undefined}
        role={isUrl ? undefined : "link"}
        tabIndex={isUrl ? undefined : 0}
        className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
        title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
        onClick={(e) => {
          e.preventDefault();
          openTarget(clean);
        }}
        onKeyDown={
          isUrl
            ? undefined
            : (e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  openTarget(clean);
                }
              }
        }
      >
        {clean}
      </a>
    );
    last = idx + token.length;
  }
  if (last < text.length) parts.push(<span key={key++}>{text.slice(last)}</span>);
  return <>{parts}</>;
}
