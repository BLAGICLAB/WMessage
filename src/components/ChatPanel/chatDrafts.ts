// ChatPanel 子模块：聊天输入区草稿的落点（文本 / 附件 / 任务引用），按会话隔离。
//
// 为什么不能只放组件 state：设置页在 App 里是**提前 return**（整棵主树被替换掉，
// 见 App.tsx「设置视图全屏接管」），聊天面板随之卸载——草稿放组件 useState 里，
// 切一次设置再切回来，输入框就是空的（用户报的「输入内容不持久」）。
// 草稿是「没发出去的东西」，不该被一次导航收走。
//
// 落盘用 localStorage（与 bubbleStyle / verbose 档位 / 会话栏折叠同范式）：
// 多扛一层「页面重载 / 应用重启」——换窗口、重启应用回来，草稿还在。
//
// 边界：只存**没发出去**的输入区内容；会话历史由后端管，这里不碰。
// 脏数据（坏 JSON / 形状不对）一律归一到空草稿，不让坏存储炸掉输入区。

import type { TaskRef } from "./types";

const KEY = "wmessage-chat-drafts";

/** 一个会话的输入区草稿（DRAFT-1：一个会话一份，互不串台） */
export type ChatDraft = {
  /** 输入框文本 */
  text: string;
  /** 待发附件路径（➕ 选 / 拖入） */
  files: string[];
  /** 待发任务引用（右侧栏点任务加入） */
  refs: TaskRef[];
};

/** 空草稿：常量引用稳定，避免每次渲染造新数组/新对象 */
export const EMPTY_DRAFT: ChatDraft = { text: "", files: [], refs: [] };

/** 空草稿不落盘——map 里不留垃圾条目（发出去就清干净） */
function isEmpty(d: ChatDraft): boolean {
  return d.text === "" && d.files.length === 0 && d.refs.length === 0;
}

/** 脏数据归一：形状不对的字段各自回退默认值（不整条丢弃，能救的救） */
function normalize(input: unknown): ChatDraft {
  if (!input || typeof input !== "object") return { ...EMPTY_DRAFT };
  const o = input as { text?: unknown; files?: unknown; refs?: unknown };
  return {
    text: typeof o.text === "string" ? o.text : "",
    files: Array.isArray(o.files)
      ? o.files.filter((f): f is string => typeof f === "string")
      : [],
    refs: Array.isArray(o.refs)
      ? o.refs.filter(
          (r): r is TaskRef =>
            !!r &&
            typeof r === "object" &&
            typeof (r as TaskRef).id === "string" &&
            typeof (r as TaskRef).title === "string",
        )
      : [],
  };
}

/**
 * 读全部草稿（面板挂载时读一次）：sessionId → 草稿。
 * 读失败 / 脏数据 → 空表（不抛：输入区必须始终能用）。
 */
export function readChatDrafts(): Map<string, ChatDraft> {
  const out = new Map<string, ChatDraft>();
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return out;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return out;
    for (const [sid, value] of Object.entries(parsed as Record<string, unknown>)) {
      const draft = normalize(value);
      if (!isEmpty(draft)) out.set(sid, draft);
    }
  } catch {
    // 坏 JSON / 无 localStorage（无头环境）→ 空草稿
  }
  return out;
}

/**
 * 写全部草稿（改动后写一次）：空草稿剪掉，其余落 localStorage。
 * 写失败（配额满 / 隐私模式）→ 静默跳过：草稿是便利功能，不该因此报错。
 */
export function writeChatDrafts(drafts: ReadonlyMap<string, ChatDraft>): void {
  try {
    const obj: Record<string, ChatDraft> = {};
    for (const [sid, draft] of drafts) {
      if (!isEmpty(draft)) obj[sid] = draft;
    }
    localStorage.setItem(KEY, JSON.stringify(obj));
  } catch {
    // ignore
  }
}
