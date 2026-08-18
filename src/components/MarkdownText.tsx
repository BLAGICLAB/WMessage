import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";

/**
 * 助手回复的 Markdown 渲染：GFM（表格/任务列表）+ 单换行断行。
 * 链接/绝对路径保持与 RichText 一致的点击行为（URL → 浏览器；路径 → 打开，失败退到 Finder 显示）。
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
          code: ({ children, className }) => {
            const text = String(children ?? "").trim();
            const isInline = !className && !text.includes("\n");
            const isUrl = /^https?:\/\//i.test(text);
            const isPath =
              /^[A-Za-z]:[\\/]/.test(text) ||
              /^\/(?:Users|home|var|tmp|Library|Applications|opt)\b/.test(text);
            if (!isInline || (!isUrl && !isPath)) {
              return <code className={className}>{children}</code>;
            }
            return (
              <code
                title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
                className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
                onClick={() => {
                  if (isUrl) {
                    openUrl(text.replace(/[.,;:!?]+$/, "")).catch(() => {});
                  } else {
                    // 挂件窗口前端 openPath 被 opener scope 拒（点击无反应）→ Rust 命令
                    invoke("open_file_path", { path: text }).catch(() => {});
                  }
                }}
              >
                {children}
              </code>
            );
          },
          a: ({ href, children }) => {
            const url = (href ?? "").trim();
            const isUrl = /^https?:\/\//i.test(url);
            const isPath =
              /^[A-Za-z]:[\\/]/.test(url) ||
              /^\/(?:Users|home|var|tmp|Library|Applications|opt)\b/.test(url);
            if (!isUrl && !isPath) return <span>{children}</span>;
            return (
              <a
                href={url}
                title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
                className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
                onClick={(e) => {
                  e.preventDefault();
                  if (isUrl) {
                    openUrl(url.replace(/[.,;:!?]+$/, "")).catch(() => {});
                  } else {
                    invoke("open_file_path", { path: url }).catch(() => {});
                  }
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
