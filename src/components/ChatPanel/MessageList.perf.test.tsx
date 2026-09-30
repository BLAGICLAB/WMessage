import { Profiler } from "react";
import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { MessageList, MessageListUnmemoized } from "./MessageList";
import type { Msg } from "./types";

// MsgBubble React.memo 流式性能对比（B5-2 验收数据）：
// 长会话（N 气泡）流式回复 = 每帧只替换最后一条消息的对象引用。
// memo 生效时历史气泡 props 全等被跳过，重渲染工作 = 仅最后一条；
// 无 memo 时每帧全列表重渲染（MarkdownText 的 markdown 解析是重活）。
// 用 React.Profiler 的 actualDuration 度量单次更新的实际渲染耗时。

const N = 30;
const UPDATES = 50;

/** 仿真实助手回复的 markdown（段落 + 列表 + 行内代码），让解析开销可测 */
function fakeMd(i: number): string {
  return [
    `这是第 ${i} 条回复的正文段落，包含一些**强调**与 \`inline code\` 文本。`,
    "",
    "- 列表项一：说明流式渲染的合并策略",
    "- 列表项二：说明 memo 边界的作用范围",
    "- 列表项三：`const x = 42;` 之类的行内代码",
    "",
    "结尾段落，保证每条消息有足量的 markdown 节点参与解析与 diff。",
  ].join("\n");
}

const stableCallbacks = {
  onCopy: () => {},
  onRemove: () => {},
  onOpenTask: () => {},
  onOpenExecSession: () => {},
};

/** 渲染 N 条消息 + UPDATES 次流式更新（只替换最后一条引用），返回每次更新的 actualDuration */
function bench(Comp: typeof MessageList): number[] {
  const base: Msg[] = Array.from({ length: N }, (_, i) => ({
    role: "assistant" as const,
    content: fakeMd(i),
  }));
  const durations: number[] = [];
  const onRender = (_id: string, _phase: string, actualDuration: number) => {
    durations.push(actualDuration);
  };
  const props = {
    scrollRef: { current: null },
    viewedBusy: false,
    copiedIdx: null,
    ...stableCallbacks,
  };
  const { rerender } = render(
    <Profiler id="ml" onRender={onRender}>
      <Comp messages={base} {...props} />
    </Profiler>,
  );
  let current = base;
  for (let u = 0; u < UPDATES; u++) {
    current = [...base];
    current[N - 1] = {
      role: "assistant",
      content: `${fakeMd(N - 1)}\n\n流式增量 chunk ${u}。`,
      streaming: true,
    };
    rerender(
      <Profiler id="ml" onRender={onRender}>
        <Comp messages={current} {...props} />
      </Profiler>,
    );
  }
  return durations.slice(1); // 去掉首次挂载（两种变体工作量相同）
}

const avg = (a: number[]) => a.reduce((s, x) => s + x, 0) / a.length;

describe("MsgBubble memo 流式性能（B5-2）", () => {
  it("流式更新只重渲染最后一条：memo 版每帧耗时显著低于无 memo 对照", () => {
    const memoDurations = bench(MessageList);
    const baseDurations = bench(MessageListUnmemoized);
    const memoAvg = avg(memoDurations);
    const baseAvg = avg(baseDurations);
    // 留档：DEVLOG 引用本数据（jsdom 数值随环境波动，倍率结论稳定）
    console.info(
      `[perf] 流式单帧渲染均值（${N} 气泡 × ${UPDATES} 次更新）：memo=${memoAvg.toFixed(2)}ms 无memo=${baseAvg.toFixed(2)}ms 加速比=${(baseAvg / memoAvg).toFixed(1)}×`,
    );
    expect(memoAvg).toBeLessThan(baseAvg);
    // 保守阈值：30 条消息的 markdown 全量重渲染至少是单条增量的 2 倍
    expect(baseAvg / memoAvg).toBeGreaterThan(2);
  });
});
