// ChatPanel 子模块：文件预览（右侧栏「文件」工作面内就地渲染）。
//
// 为什么在侧栏里看而不是跳出去：文件行来自本会话的改动记录，用户点它几乎
// 都是为了「确认刚才改成了什么样」。跳到系统默认程序就得切窗口、看完再切回
// 聊天，上下文整个断掉。留在侧栏里，对话不动——与「子 Agent」工作面点条目
// 进详情是同一套「列表 → 详情 → 返回」的手感。
//
// 只读文本：二进制文件由后端判掉（前端不猜），超限由后端截断并标出来——
// 截断了还让用户以为这就是全文，等于给一份不完整的证据。

import { useEffect, useState } from "react";
import { ArrowLeft, FileText, FolderOpen, Loader2, TriangleAlert } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { MarkdownText } from "../MarkdownText";
import { formatCommandError, handleCommandError } from "../../lib/errorHandler";

/** 后端 `bot_preview_text` 载荷（camelCase 镜像） */
type TextPreview = {
  path: string;
  content: string;
  truncated: boolean;
  totalBytes: number;
  lines: number;
};

/** markdown 扩展名（小写比对）：这些走 Markdown 渲染，其余按纯文本原样显示。
 *  判扩展名而不是嗅探内容——markdown 的表格/标题排版是给人看的，
 *  按扩展名认主流写法够用，也不必在 rust 侧复制一份规则。 */
const MARKDOWN_EXT = new Set([".md", ".markdown", ".mdx"]);

function isMarkdown(path: string): boolean {
  const i = path.lastIndexOf(".");
  if (i < 0) return false;
  return MARKDOWN_EXT.has(path.slice(i).toLowerCase());
}

export function FilePreview({
  path,
  added,
  deleted,
  onBack,
}: {
  path: string;
  added: number;
  deleted: number;
  onBack: () => void;
}) {
  const [data, setData] = useState<TextPreview | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    // 换文件时靠父级 key 重挂载拿到干净的空状态（见 ContextRail 的 key={path}），
    // 不在这里同步 setState 清空：那会多渲一轮，且 effect 本就只在挂载时跑一次
    let alive = true;
    invoke<TextPreview>("bot_preview_text", { path })
      .then((d) => {
        if (alive) setData(d);
      })
      .catch((e) => {
        // 失败必须显形：空内容会被读成「文件是空的」，比报错更误导。
        // silent = 只 console 不弹窗——错误就摆在面板上，再弹一次是双重提示
        if (alive) {
          handleCommandError(e, "预览文件", { silent: true });
          setErr(formatCommandError(e) || "读取失败");
        }
      });
    return () => {
      alive = false;
    };
  }, [path]);

  const name = path.split(/[\\/]/).pop() ?? path;
  return (
    <div className="flex min-h-0 flex-col">
      {/* 返回行与子 Agent 详情同款：详情是一层，返回才回列表 */}
      <button
        type="button"
        onClick={onBack}
        className="mb-1 inline-flex items-center gap-1 text-[11px] text-[var(--t4)] hover:text-[var(--t2)]"
      >
        <ArrowLeft size={11} aria-hidden /> 返回文件列表
      </button>
      <div className="mb-1.5 flex items-start gap-1.5 px-0.5">
        <FileText
          size={12}
          aria-hidden
          className="mt-0.5 shrink-0 text-[var(--t5)]"
        />
        <div className="min-w-0 flex-1">
          <p className="truncate text-[11px] font-medium text-[var(--t2)]" title={path}>
            {name}
          </p>
          <p className="truncate text-[10px] text-[var(--t5)]" title={path}>
            {path}
          </p>
          <p className="font-mono text-[10px] text-[var(--t5)]">
            <span className="text-[var(--success)]">+{added}</span>{" "}
            <span className="text-[var(--danger)]">−{deleted}</span>
          </p>
        </div>
      </div>

      {err && (
        <p className="flex items-start gap-1 px-0.5 text-[11px] text-[var(--danger)]">
          <TriangleAlert size={11} aria-hidden className="mt-0.5 shrink-0" />
          {err}
        </p>
      )}
      {!err && !data && (
        <p className="inline-flex items-center gap-1 px-0.5 text-[11px] text-[var(--t5)]">
          <Loader2 size={11} aria-hidden className="animate-spin" />
          读取中…
        </p>
      )}
      {data && (
        <>
          {data.truncated && (
            <p className="mb-1 rounded-md px-1.5 py-1 text-[10px] text-[var(--t5)] nm-inset">
              文件较大，只显示前 {data.content.length} 字节 / 共{" "}
              {data.totalBytes} 字节
            </p>
          )}
          <div
            className="min-h-0 flex-1 overflow-y-auto rounded-lg nm-card px-2 py-1.5 text-[11px] leading-relaxed"
            aria-label="文件内容"
          >
            {isMarkdown(data.path) ? (
              <MarkdownText text={data.content} />
            ) : (
              <pre className="whitespace-pre-wrap break-words font-mono text-[10px] leading-relaxed text-[var(--t3)]">
                {data.content}
              </pre>
            )}
          </div>
        </>
      )}
      <p className="mt-1.5 px-0.5 text-[10px] text-[var(--t5)]">
        <FolderOpen size={10} aria-hidden className="mr-1 inline" />
        预览为只读快照；要看实时内容请在外部编辑器打开
      </p>
    </div>
  );
}
