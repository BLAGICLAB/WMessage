// DOM 危险通道守卫：生产代码不得把内容当标记解析。
//
// 这些口子一旦接上不可信数据就是存储型注入面。任务标题/标签/备注的来源
// 包括 bot 写卡、外来 tasks.json 信封、HTTP API —— 都不受信。
// 文案、工具入参、结果这类地方一律走文本节点（React 自动转义）。
//
// 守卫扫的是 src/ 下的生产代码（测试文件自身的字面量不算）。
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

function prodFiles(): string[] {
  return readdirSync("src", { recursive: true })
    .map(String)
    .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
}

const SINKS: Array<[string, RegExp]> = [
  ["dangerouslySetInnerHTML", /dangerouslySetInnerHTML/],
  ["innerHTML", /\.innerHTML\s*=/],
  ["outerHTML", /\.outerHTML\s*=/],
  ["insertAdjacentHTML", /\.insertAdjacentHTML\s*\(/],
  ["document.write", /document\.write(ln)?\s*\(/],
  ["srcDoc", /\.srcDoc\s*=/],
  ["iframe", /<iframe\b/],
  ["DOMParser", /new\s+DOMParser\b/],
  ["eval", /\beval\s*\(/],
  ["new Function", /new\s+Function\b/],
];

describe("DOM 危险通道守卫", () => {
  for (const [name, re] of SINKS) {
    it(`生产代码零命中：${name}`, () => {
      const hits: string[] = [];
      for (const f of prodFiles()) {
        const src = readFileSync(join("src", f), "utf-8");
        if (re.test(src)) hits.push(f);
      }
      expect(hits, `${name} 不该出现在生产代码里`).toEqual([]);
    });
  }
});
