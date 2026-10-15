import { useState } from "react";
import { Folder, Paperclip } from "lucide-react";
import type { Task } from "../../types";
import { basename } from "../../format";
import { taskFiles } from "../../lib/taskFiles";
import { stopPointer } from "../../lib/stopPointer";

/** 任务卡绑定文件 chip 列表——两处复用的统一渲染：
 *  - compact：主窗口卡片（TodoCard），lucide 小图标 + 11px 字号
 *  - spacious：任务页详情面板（TaskDetailPanel），emoji + 较大 padding + 复制反馈徽标
 *  - 折叠/展开「还有 N 个」按钮内置；绑定操作行（pickFile/解绑全部）由调用方挂在同一父容器里
 *  - 仅渲染 taskFiles(task).length>0；空数组由调用方决定是否渲染空态绑定按钮
 */
export interface TaskFileChipsProps {
  task: Task;
  /** 视觉样式；默认 compact（卡片用 compact，详情面板用 spacious） */
  variant?: "compact" | "spacious";
  /** 点 chip 名/文件夹名直接打开；不传则 chip 渲染为只读文件名 */
  onOpenPath?: (path: string) => void;
  /** chip 内「复制」字样回调；不传则不渲染复制按钮 */
  onCopyPath?: (path: string) => void;
  /** chip 内「×」移除按钮回调；不传则不渲染 ×（只读场景） */
  onRemovePath?: (path: string) => void;
  /** × 按钮 title；详情面板用 "移除绑定"，默认 "移除该文件" */
  removeTitle?: string;
  /** 复制反馈态：路径 → ok/err；非空时显示徽标替代「复制」按钮（详情面板专用） */
  copyState?: Record<string, "ok" | "err">;
}

export function TaskFileChips({
  task,
  variant = "compact",
  onOpenPath,
  onCopyPath,
  onRemovePath,
  removeTitle = "移除该文件",
  copyState,
}: TaskFileChipsProps) {
  const boundFiles = taskFiles(task);
  const [filesExpanded, setFilesExpanded] = useState(false);
  // 空绑定：返回 null，调用方负责空态「绑定文件/绑定文件夹」按钮
  if (boundFiles.length === 0) return null;

  const isSpacious = variant === "spacious";

  return (
    <div
      className={
        isSpacious ? "flex flex-col items-start gap-1" : "flex flex-col gap-1"
      }
      // aria-live 给复制反馈用的——只有传了 copyState 才需要
      {...(copyState ? { "aria-live": "polite" as const } : {})}
    >
      {(filesExpanded ? boundFiles : boundFiles.slice(0, 5)).map((f) => {
        const showOpenBtn = !!onOpenPath;
        const showCopyBtn = !!onCopyPath && !(isSpacious && copyState?.[f.path]);
        const showRemoveBtn = !!onRemovePath;
        // chip 容器：详情面板用更宽松 padding + text-xs；卡片用紧凑样式
        const chipClass = isSpacious
          ? "nm-inset flex max-w-full items-center gap-1 px-2 py-0.5 text-xs"
          : "nm-inset rounded-lg px-1.5 py-0.5 flex items-center gap-1.5";
        // 打开按钮样式：详情面板限定宽度 10rem，卡片 flex-1 占满
        const openClass = isSpacious
          ? "max-w-40 truncate text-[var(--t4)] hover:text-[var(--t2)]"
          : "flex-1 min-w-0 text-left text-[11px] text-[var(--t4)] hover:text-[var(--t2)] truncate";

        return (
          <div key={f.path} className={chipClass}>
            {showOpenBtn ? (
              <button
                type="button"
                className={openClass}
                title={isSpacious ? f.path : `${f.path}（点击打开）`}
                onPointerDown={isSpacious ? undefined : stopPointer}
                onClick={() => {
                  onOpenPath!(f.path);
                }}
              >
                {isSpacious ? (
                  <>
                    {f.isDir ? "📁 " : "📄 "}
                    {basename(f.path)}
                  </>
                ) : (
                  <>
                    {f.isDir ? (
                      <Folder
                        size={11}
                        aria-hidden
                        className="inline-block align-[-2px]"
                      />
                    ) : (
                      <Paperclip
                        size={11}
                        aria-hidden
                        className="inline-block align-[-2px]"
                      />
                    )}{" "}
                    {basename(f.path)}
                  </>
                )}
              </button>
            ) : (
              // 卡片：未接线 onOpenPath 时退化为只读文字
              <p
                className="flex-1 min-w-0 text-[11px] text-[var(--t4)] truncate"
                title={f.path}
              >
                {f.isDir ? (
                  <Folder
                    size={11}
                    aria-hidden
                    className="inline-block align-[-2px]"
                  />
                ) : (
                  <Paperclip
                    size={11}
                    aria-hidden
                    className="inline-block align-[-2px]"
                  />
                )}{" "}
                {basename(f.path)}
              </p>
            )}
            {/* 复制按钮 / 复制反馈徽标二选一：详情面板在反馈态用徽标替代按钮 */}
            {isSpacious && copyState?.[f.path] ? (
              <span
                className={`shrink-0 text-[10px] font-medium ${
                  copyState[f.path] === "ok"
                    ? "text-[var(--success)]"
                    : "text-[var(--danger)]"
                }`}
              >
                {copyState[f.path] === "ok" ? "✓ 已复制" : "复制失败"}
              </span>
            ) : showCopyBtn ? (
              <button
                type="button"
                className={`shrink-0 text-[10px] text-[var(--t5)] ${
                  isSpacious
                    ? "hover:text-[var(--t2)]"
                    : "hover:text-[var(--t3)]"
                }`}
                title={isSpacious ? "复制到剪贴板" : "复制文件+标题"}
                onPointerDown={isSpacious ? undefined : stopPointer}
                onClick={() => {
                  onCopyPath!(f.path);
                }}
              >
                复制
              </button>
            ) : null}
            {showRemoveBtn && (
              <button
                type="button"
                className={`shrink-0 text-[var(--t5)] hover:text-[var(--danger)] ${
                  isSpacious ? "" : "text-sm"
                }`}
                title={removeTitle}
                {...(isSpacious
                  ? { "aria-label": `移除 ${basename(f.path)}` }
                  : {})}
                onPointerDown={isSpacious ? undefined : stopPointer}
                onClick={() => {
                  onRemovePath!(f.path);
                }}
              >
                ×
              </button>
            )}
          </div>
        );
      })}
      {boundFiles.length > 5 && (
        <button
          type="button"
          className="self-start text-[11px] text-[var(--t5)] hover:text-[var(--t3)]"
          onPointerDown={isSpacious ? undefined : stopPointer}
          onClick={() => {
            setFilesExpanded((v) => !v);
          }}
        >
          {filesExpanded ? "收起" : `还有 ${boundFiles.length - 5} 个`}
        </button>
      )}
    </div>
  );
}
