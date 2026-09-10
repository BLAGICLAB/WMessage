import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "./errorHandler";

/**
 * 链接/路径识别与打开的统一入口（彻底修复「聊天下方文档/网址链接
 * 有时打不开、有时显示 Program」）：
 * 根因一：路径正则按空白截断——「C:\Program Files\...」「报告 终稿.docx」这类带空格
 *   路径被切成空格前一段，链接显示成「Program」、点击打开一个不存在的路径；
 * 根因二：所有点击失败都 silent catch，用户分不清是没点上还是打不开；
 * 根因三：looksLikeUrl 把「C:」误判为 URL scheme，Windows 路径被存成网址链接后
 *   走 openUrl 被 opener scope 拒绝。
 * 统一约定：按内容判定（不信任存储的 kind）+ 带空格路径识别（锚定扩展名）+ 失败可见。
 */

/** 已生成文档/常见文件扩展名（带空格路径必须锚定到扩展名，防贪婪吞掉后续正文） */
const FILE_EXTS =
  "(?:docx?|xlsx?|pptx?|pdf|markdown|md|txt|csv|json|xml|ya?ml|log|html?|png|jpe?g|gif|webp|svg|bmp|zip|rar|7z|tar|gz|mp[34]|wav|mov)";

/** 路径前缀：Windows 盘符 或 常见 unix 绝对路径根 */
const PATH_PREFIX =
  "(?:[A-Za-z]:[\\\\/]|\\/(?:Users|home|var|tmp|Library|Applications|opt)\\b)";

/** 路径字符（不含空格）：排除引号/反引号/CJK 标点 */
const PATH_CHAR_NOSPACE = "[^\\s<>\"'`，。；、（）【】！!？?*]";
/** 路径字符（允许空格与中文）：排除换行/引号/反引号/CJK 标点 */
const PATH_CHAR_SPACE = "[^<>\"'`，。；、（）【】！!？?*|\\n]";

/**
 * 文本内 URL / 绝对路径识别（全局匹配）。
 * URL 分支允许 ASCII ? !（查询串不再被截断；句尾标点由 normalizeUrl/clean 剥离），
 * 全角标点仍作边界。
 * 路径两个分支：带空格路径惰性锚定到已知扩展名；无空格路径（文件夹/无扩展名）保持旧行为。
 * 带扩展名的无空格路径会优先命中第二分支（首个扩展名处截断），行为一致。
 */
export const LINK_OR_PATH_RE = new RegExp(
  "(https?:\\/\\/[^\\s<>\"'，。；、（）【】！？`*]+)" +
    `|(${PATH_PREFIX}${PATH_CHAR_SPACE}*?\\.${FILE_EXTS}\\b)` +
    `|(${PATH_PREFIX}${PATH_CHAR_NOSPACE}*)`,
  "gi"
);

export const isHttpUrl = (s: string) => /^https?:\/\//i.test(s);

/** 裸域名（www.example.com / example.com/path）：补 https:// 后可开 */
const BARE_DOMAIN_RE = /^(?:[a-z0-9-]+\.)+[a-z]{2,}(?:[/?#]|$)/i;

/** 绝对路径判定（与 LINK_OR_PATH_RE 前缀一致） */
export const isAbsPath = (s: string) =>
  new RegExp(`^${PATH_PREFIX}`).test(s);

/**
 * 从消息文本提取文件路径（绝对路径，去重保序）：供消息下方 📄 文件按钮。
 * 机器人产物的路径可能被反引号包裹（Markdown code），正则穿过反引号提取后清理。
 */
export function extractFilePaths(content: string): string[] {
  const out: string[] = [];
  for (const m of content.matchAll(LINK_OR_PATH_RE)) {
    const token = m[0];
    if (isHttpUrl(token)) continue;
    const p = token.replace(/[`.,;:!?]+$/, "").trim();
    if (!p || out.includes(p)) continue;
    out.push(p);
  }
  return out;
}

/** URL 规范化：剥离尾随 ASCII 标点；裸域名补 https://；无法规范化返回 null */
export function normalizeUrl(raw: string): string | null {
  const s = raw.trim().replace(/[.,;:!?]+$/, "");
  if (isHttpUrl(s) || /^(mailto:|tel:)/i.test(s)) return s;
  if (BARE_DOMAIN_RE.test(s)) return `https://${s}`;
  return null;
}

/**
 * 统一打开入口（按内容判定，不信任存储的 kind——历史数据可能 kind 错配）：
 * URL（裸域名补 scheme）→ 浏览器；绝对路径 → Rust open_file_path（绕 opener scope）。
 * 失败弹错不静默——「看到原因」比「点了没反应」可排查。
 */
export function openTarget(raw: string): void {
  const target = raw.trim();
  const url = normalizeUrl(target);
  if (url) {
    openUrl(url).catch((e) => handleCommandError(e, "open url"));
    return;
  }
  if (isAbsPath(target)) {
    invoke("open_file_path", { path: target }).catch((e) =>
      handleCommandError(e, "open_file_path")
    );
    return;
  }
  handleCommandError(
    { code: "INVALID_ARGUMENT", message: `无法识别的链接目标：${target}`, recoverable: false },
    "open link"
  );
}
