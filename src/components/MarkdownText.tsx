import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import { isAbsPath, isHttpUrl, openTarget } from "../lib/openTarget";

/**
 * 助手回复的 Markdown 渲染：GFM（表格/任务列表）+ 单换行断行。
 * 链接/绝对路径保持与 RichText 一致的点击行为（统一走 openTarget：
 * URL → 浏览器；路径 → Rust open_file_path；失败弹错不静默）。
 * react-markdown 默认转义原始 HTML，安全。
 */
export function MarkdownText({ text }: { text: string }) {
  return (
    <div className="md-body">
      <Markdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        components={{
          // 行内代码：反引号包裹的路径/URL 也渲染成可点链接
          // （模型输出路径常带反引号，此前变成纯代码文本点不开）
          code: ({ children, className, node }) => {
            // 仅纯文本 children 参与链接化——数组 children 经 String() 会以 ","
            // join 产出 "foo,bar"，可能把无害代码段误判成 URL/路径路由进 openTarget
            let linkText = "";
            if (typeof children === "string") {
              linkText = children.trim();
            } else if (
              Array.isArray(children) &&
              children.every((c) => typeof c === "string")
            ) {
              linkText = children.join("").trim();
            }
            // react-markdown 会剥掉围栏代码块内容的尾换行，无语言标记的单行围栏块
            // 用文本启发式（!className && 无换行）会误判为行内；改用 node.position：
            // 围栏块的 code 节点 position 覆盖围栏行（start/end 跨行），行内代码不跨行
            const isBlock =
              node?.position != null &&
              node.position.start.line !== node.position.end.line;
            const isInline = !isBlock && !className && !linkText.includes("\n");
            const isUrl = isHttpUrl(linkText);
            const isPath = isAbsPath(linkText);
            if (!isInline || (!isUrl && !isPath)) {
              return <code className={className}>{children}</code>;
            }
            // 渲染 <a> 而非带 onClick 的 <code>：键盘可聚焦 + 屏幕阅读器播报为链接。
            // 路径链接不设 href（会被中键/复制链接当 URL 解析出 bogus 地址）；
            // 无 href 的 <a> 不可聚焦，补 role="link" + tabIndex + Enter 触发
            const activate = () => openTarget(linkText);
            return (
              <a
                href={isUrl ? linkText : undefined}
                role={isUrl ? undefined : "link"}
                tabIndex={isUrl ? undefined : 0}
                title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
                className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
                onClick={(e) => {
                  e.preventDefault();
                  activate();
                }}
                onKeyDown={
                  isUrl
                    ? undefined
                    : (e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          activate();
                        }
                      }
                }
              >
                {children}
              </a>
            );
          },
          a: ({ href, children }) => {
            const url = (href ?? "").trim();
            const isUrl = isHttpUrl(url);
            const isPath = isAbsPath(url);
            if (!isUrl && !isPath) return <span>{children}</span>;
            return (
              <a
                href={url}
                title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
                className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
                onClick={(e) => {
                  e.preventDefault();
                  openTarget(url);
                }}
              >
                {children}
              </a>
            );
          },
        }}
      >
        {text}
      </Markdown>
    </div>
  );
}
