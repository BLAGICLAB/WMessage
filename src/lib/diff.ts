// unified diff 的着色与分段（纯函数，无 React / 无 IO）。
//
// 为什么前端要解析 unified diff：后端给的是「改动块 + 少量上下文」，两段改动
// 块之间被省略的未改动行在文本里根本没有——要把「省略了多少」告诉用户，得从
// hunk 头的行号算出来（git/diff 查看器的老办法）。
//
// 行号语义（git 口径，已在 bot_fs 的测试里钉住）：hunk 头的计数**含上下文行**，
// 故「下一段起始旧行 − 本段起始旧行 − 本段计数」= 中间省略的旧行数。

/** hunk 头：@@ -oldStart[,oldCount] +newStart[,newCount] @@ */
const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** 单行着色类名：文件头弱化 / hunk 头高亮 / +/- 红绿 / 上下文原样 */
export function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---"))
    return "text-[var(--t5)]";
  if (line.startsWith("@@")) return "text-[var(--info,#3b82f6)]";
  if (line.startsWith("+"))
    return "bg-[var(--ok,#22c55e)]/10 text-[var(--ok,#22c55e)]";
  if (line.startsWith("-"))
    return "bg-[var(--danger,#ef4444)]/10 text-[var(--danger,#ef4444)]";
  return "";
}

/** diff 的一个片段：连续若干行，或一段被省略的未改动行 */
export type DiffSegment =
  | { type: "lines"; lines: string[] }
  | { type: "gap"; omitted: number };

/**
 * 把 unified diff 拆成「行片段 + 省略段」。
 *
 * 首段之前省略的行数 = 起始旧行 − 1；末段之后省略多少行看不出来（diff 文本里
 * 没有文件总行数），不猜——猜出来的数字比没有更糟。
 */
export function parseDiffSegments(diff: string): DiffSegment[] {
  const segments: DiffSegment[] = [];
  let buf: string[] = [];
  // 上一段覆盖到的旧行号（不含）：下一段的起始旧行减它就是中间的省略行数
  let prevEnd: number | null = null;
  const flush = () => {
    if (buf.length > 0) {
      segments.push({ type: "lines", lines: buf });
      buf = [];
    }
  };
  const rawLines = diff.split("\n");
  // 尾换行切出来的空串不是 diff 行（后端拼文本用 \n join，不留尾换行）
  if (rawLines.length > 0 && rawLines[rawLines.length - 1] === "") rawLines.pop();
  for (const line of rawLines) {
    const m = HUNK_RE.exec(line);
    if (!m) {
      buf.push(line);
      continue;
    }
    const oldStart = Number(m[1]);
    const oldCount = m[2] === undefined ? 1 : Number(m[2]);
    const gapBefore = prevEnd === null ? oldStart - 1 : oldStart - prevEnd;
    flush();
    if (gapBefore > 0) segments.push({ type: "gap", omitted: gapBefore });
    buf.push(line);
    prevEnd = oldStart + oldCount;
  }
  flush();
  return segments;
}

/** 省略段的提示文案（行数是事实：这些行确实没显示） */
export function gapLabel(omitted: number): string {
  return `⋯ 省略 ${omitted} 行未改动`;
}
