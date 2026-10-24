import { describe, expect, it } from "vitest";
import {
  ACTIVITY_MAX_ITEMS,
  applyActivityEvent,
  type SubagentActivityEvent,
  type SubagentActivityItem,
} from "./subagentActivity";

const call = (toolCallId: string, tool: string, preview = ""): SubagentActivityEvent => ({
  subagentId: "sa_1",
  turn: 3,
  toolCallId,
  tool,
  phase: "call",
  ok: null,
  ms: null,
  preview,
});

const ret = (
  toolCallId: string,
  tool: string,
  ok: boolean,
  ms = 8,
  preview = "done",
): SubagentActivityEvent => ({
  subagentId: "sa_1",
  turn: 3,
  toolCallId,
  tool,
  phase: "return",
  ok,
  ms,
  preview,
});

describe("applyActivityEvent", () => {
  it("call 开新行（进行中，ok=null）", () => {
    const out = applyActivityEvent([], call("t1", "read_text_file", '{"path":"SPEC.md"}'));
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({
      toolCallId: "t1",
      tool: "read_text_file",
      ok: null,
      ms: null,
    });
  });

  it("return 按 toolCallId 收口 call 行：同位补结局，不再多一行", () => {
    let out = applyActivityEvent([], call("t1", "grep_files"));
    out = applyActivityEvent(out, ret("t1", "grep_files", true, 12, "3 hits"));
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ ok: true, ms: 12, preview: "3 hits" });
  });

  it("return 找不到对应 call（乱序/重启）→ 当已完成动作入列头", () => {
    const existing: SubagentActivityItem[] = [
      { toolCallId: "t1", tool: "a", ok: null, ms: null, preview: "", at: 1 },
    ];
    const out = applyActivityEvent(existing, ret("t9", "b", false));
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ tool: "b", ok: false });
    expect(out[1]).toMatchObject({ tool: "a", ok: null });
  });

  it("列表恒持最近 N 条，新在头", () => {
    let out: SubagentActivityItem[] = [];
    for (let i = 0; i < ACTIVITY_MAX_ITEMS + 3; i++) {
      out = applyActivityEvent(out, call(`t${i}`, `tool_${i}`));
    }
    expect(out).toHaveLength(ACTIVITY_MAX_ITEMS);
    expect(out[0].tool).toBe(`tool_${ACTIVITY_MAX_ITEMS + 2}`);
  });
});
