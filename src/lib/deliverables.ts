import { invoke } from "@tauri-apps/api/core";
import { extractFilePaths, FILE_EXTS } from "./openTarget";

/**
 * 交付物清单（聊天交付物区 + 右侧栏「交付物」页共用一套口径）。
 *
 * 语义：交付物 = **最终产物**，不是执行过程文件。系统提示词要求机器人
 * 「给用户的汇报里只列最终交付物：中间草稿、脚本、临时文件一律不列」——
 * 所以正文里点名的路径，就是机器人自己认定的交付物清单，机器不再替它挑。
 *
 * 两个来源：
 * - declared：正文自报（模型点名）。**不过形态过滤**：模型点名了就是它的
 *   交付物判断，配套说明文档（.md）同样算交付物。
 * - produced：本轮/本会话新写出来的文件。**过形态过滤**：写盘通道记的是
 *   「动过的文件」，里面混着生成脚本等中间产物形态，挂在交付物位上会淹没
 *   真正要拿的那份。
 */

/** 交付物条目 */
export type Deliverable = {
  /** 绝对路径（用于打开/预览） */
  path: string;
  /** 来源：正文自报 / 新产生 */
  source: "declared" | "produced";
};

/**
 * 交付物形态白名单：复用链接识别那套扩展名清单（含 pptx/docx/xlsx/pdf/md/
 * 图片/压缩包，**不含 py/js/sh 这类脚本形态**）。
 * 不另立一份规则——两份清单必然漂移，漂移的方向是「交付物区漏挂」。
 */
const DELIVERABLE_EXT_RE = new RegExp(`\\.${FILE_EXTS}$`, "i");

/** 路径查重键：与任务卡绑定同口径（小写归一，同名不同路径视为不同条目） */
function normalizePathKey(path: string): string {
  return path.trim().toLowerCase();
}

/** 是不是交付物形态（扩展名在白名单里） */
function isDeliverableShape(path: string): boolean {
  return DELIVERABLE_EXT_RE.test(path.trim());
}

/** 从新产生的一批路径里挑出交付物形态的（脚本等中间产物形态挡掉） */
export function pickDeliverableShaped(paths: string[]): string[] {
  return paths.filter(isDeliverableShape);
}

/**
 * 每条消息的正文路径抽取结果缓存（键 = 消息对象）。
 *
 * 键用**对象身份**而不是内容：流式输出每来一个 token 就换一个新的消息
 * 对象，历史消息的对象不变——按内容做键得把全会话正文重新比一遍，按对象
 * 做键只重扫正在流的那一条（与气泡内的算法同量）。
 */
const declaredByMessage = new WeakMap<object, string[]>();

/**
 * 会话级「正文自报」路径汇总（右侧栏「交付物」页的 declared 半边）。
 *
 * 只收助手消息：用户消息里的路径是**引用**（让它读那个文件），不是交付。
 * 去重按路径小写归一，与 `mergeDeliverables` 同口径。
 */
export function collectDeclaredPaths(
  messages: { role: string; content: string }[],
): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const m of messages) {
    if (m.role !== "assistant") continue;
    let paths = declaredByMessage.get(m);
    if (!paths) {
      paths = extractFilePaths(m.content);
      declaredByMessage.set(m, paths);
    }
    for (const p of paths) {
      const key = normalizePathKey(p);
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(p);
    }
  }
  return out;
}

/**
 * 合并两个来源：declared 在前（正文出现顺序 = 机器人汇报顺序），produced
 * 追加在后（写盘顺序）。
 *
 * **produced 侧自动过形态过滤**：写盘通道记的是「动过的文件」，里面混着
 * 生成脚本等中间产物形态。过滤内建在这里而不是留给调用方——调用方漏调
 * 一次就是脚本漏挂进交付物位，正是这个功能要治的毛病。
 *
 * 同路径只留一条，先到的赢（declared 优先）——它是机器人明确点名的，比
 * 「写盘通道里出现过」更能代表交付物。
 */
export function mergeDeliverables(
  declared: string[],
  produced: string[],
): Deliverable[] {
  const out: Deliverable[] = [];
  const seen = new Set<string>();
  const push = (path: string, source: Deliverable["source"]) => {
    const key = normalizePathKey(path);
    if (!key || seen.has(key)) return;
    seen.add(key);
    out.push({ path, source });
  };
  for (const p of declared) push(p, "declared");
  for (const p of pickDeliverableShaped(produced)) push(p, "produced");
  return out;
}

/** 后端路径探测结果（`bot_probe_paths` 返回） */
export type PathProbe = {
  /** 原样回显的路径 */
  path: string;
  /** 磁盘上是否存在 */
  exists: boolean;
  /** 是目录 */
  isDir: boolean;
  /** 文件字节数（目录为 0） */
  size: number;
};

/** 单批探测的条数上限：与后端 `bot_probe_paths` 的参数上限一致 */
const PROBE_BATCH_MAX = 500;

/**
 * 批量探测路径是否存在。存在性判定是同步 syscall，在 Rust 侧 blocking 池
 * 跑（前端没有 stat 能力，也不能拿 opener 的报错当判据）。
 *
 * 分批发送：后端单次上限 500 条，长会话的交付物清单能超。超限直接发会被
 * 后端判参数越界、整批探测作废——分批后每一批都在上限内。
 */
export function probePaths(paths: string[]): Promise<PathProbe[]> {
  if (paths.length === 0) return Promise.resolve([]);
  const batches: string[][] = [];
  for (let i = 0; i < paths.length; i += PROBE_BATCH_MAX) {
    batches.push(paths.slice(i, i + PROBE_BATCH_MAX));
  }
  return Promise.all(
    batches.map((b) => invoke<PathProbe[]>("bot_probe_paths", { paths: b })),
  ).then((rs) => rs.flat());
}

/**
 * 交付物清单收敛成「磁盘上真在」的那些。
 *
 * 不存在的路径**剔掉而不是照挂**：挂一个点不开的条目正是这个功能要治的
 * 毛病——用户点下去才发现路径没了，比一开始没给更挫败。
 */
export function keepExisting(
  items: Deliverable[],
  probes: PathProbe[],
): Deliverable[] {
  const alive = new Set(
    probes.filter((p) => p.exists).map((p) => normalizePathKey(p.path)),
  );
  return items.filter((d) => alive.has(normalizePathKey(d.path)));
}
