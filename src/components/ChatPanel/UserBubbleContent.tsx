// ChatPanel 子模块：用户消息气泡内容（附件芯片 + 正文）。
// 图片用 🖼️ 标记（与后端 IMAGE_EXTS 对齐）；其他用 📎。

import { basename } from "../../format";

/** 图片扩展名白名单（与 Rust bot_chat.rs::IMAGE_EXTS 对齐；后端 attach_images
 *  按同一列表判断是否转 base64 image_url）。改动需两侧同步。 */
const IMAGE_EXTS = ["png", "jpg", "jpeg", "webp", "gif", "bmp"] as const;
const IMAGE_EXT_SET = new Set<string>(IMAGE_EXTS);

/** 路径后缀是否图片类型（大小写不敏感）。无后缀或未知后缀按文件处理。 */
export function isImagePath(p: string): boolean {
  const m = p.toLowerCase().match(/\.([a-z0-9]+)$/);
  return m ? IMAGE_EXT_SET.has(m[1]) : false;
}

/** 从消息内容里拆出 [附件文件] 块（历史消息恢复附件芯片显示用） */
function splitAttachments(content: string): { files: string[]; text: string } {
  const m = content.match(/^\[附件文件\]\n((?:- .+\n)+)\n/);
  if (!m) return { files: [], text: content };
  const files = m[1]
    .split("\n")
    .filter((l) => l.startsWith("- "))
    .map((l) => l.slice(2));
  return { files, text: content.slice(m[0].length) };
}

/** 用户消息气泡：附件块渲染成 📎/🖼️ 芯片 + 正文。图片用 🖼️ 标记（与后端 IMAGE_EXTS 对齐）。 */
export function UserBubbleContent({ content }: { content: string }) {
  const { files, text } = splitAttachments(content);
  return (
    <>
      {files.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-1">
          {files.map((f) => (
            <span
              key={f}
              className="nm-inset inline-flex items-center gap-1 rounded-lg px-1.5 py-0.5 text-[10px] text-[var(--t4)] max-w-full"
              title={f}
            >
              <span className="truncate max-w-[220px]">
                {isImagePath(f) ? "🖼️" : "📎"} {basename(f)}
              </span>
            </span>
          ))}
        </div>
      )}
      {text}
    </>
  );
}
