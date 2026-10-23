import { describe, it, expect, beforeEach } from "vitest";
import * as streamState from "./streamState";

beforeEach(() => {
  streamState.resetAll();
});

describe("streamState", () => {
  // 序号去重闸是「补齐与实时重叠」的唯一防线：同一条事件两边各来一次时，
  // 第二次必须被挡住——不然正文会叠成两半（用户看到「重说了一遍」）。
  it("序号去重：同号第二次放行失败，序号前进后放行", () => {
    expect(streamState.noteSeq("s1", 1)).toBe(true);
    expect(streamState.noteSeq("s1", 1)).toBe(false);
    expect(streamState.noteSeq("s1", 2)).toBe(true);
    expect(streamState.noteSeq("s1", 2)).toBe(false);
    expect(streamState.seqOf("s1")).toBe(2);
  });

  it("序号去重按会话隔离：A 会话的序号不影响 B", () => {
    expect(streamState.noteSeq("s1", 5)).toBe(true);
    expect(streamState.noteSeq("s2", 5)).toBe(true);
    expect(streamState.seqOf("s1")).toBe(5);
    expect(streamState.seqOf("s2")).toBe(5);
  });

  it("无序号的事件一律放行（老后端 / 非会话事件）", () => {
    expect(streamState.noteSeq("s1", undefined)).toBe(true);
    expect(streamState.noteSeq("s1", null)).toBe(true);
    expect(streamState.seqOf("s1")).toBe(0);
  });

  it("游标归零：回灌整轮前要把上一轮的游标清掉", () => {
    streamState.noteSeq("s1", 42);
    expect(streamState.seqOf("s1")).toBe(42);
    streamState.resetCursor("s1");
    expect(streamState.seqOf("s1")).toBe(0);
    expect(streamState.noteSeq("s1", 43)).toBe(true);
  });

  it("在途集合：置位 / 收尾 / 订阅通知", () => {
    const seen: Array<ReadonlySet<string>> = [];
    const off = streamState.subscribeInflight((sids) => seen.push(sids));
    streamState.markInflight("s1");
    expect(streamState.isInflight("s1")).toBe(true);
    expect(streamState.inflightSessions().has("s1")).toBe(true);
    streamState.markFinished("s1");
    expect(streamState.isInflight("s1")).toBe(false);
    expect(seen.length).toBe(2);
    expect(seen[0].has("s1")).toBe(true);
    expect(seen[1].has("s1")).toBe(false);
    off();
    streamState.markInflight("s2");
    expect(seen.length).toBe(2);
  });

  it("删会话：在途态、游标、回灌标记一并清干净", () => {
    streamState.markInflight("s1");
    streamState.markResumed("s1");
    streamState.noteSeq("s1", 9);
    streamState.ensureMeta("s1").text = "半截";
    streamState.dropSession("s1");
    expect(streamState.isInflight("s1")).toBe(false);
    expect(streamState.isResumed("s1")).toBe(false);
    expect(streamState.seqOf("s1")).toBe(0);
    expect(streamState.metaOf("s1")).toBeUndefined();
  });

  it("meta：取不到时建一个空的，取到时是同一份", () => {
    const m = streamState.ensureMeta("s1");
    m.text = "第一段";
    expect(streamState.metaOf("s1")?.text).toBe("第一段");
    expect(streamState.ensureMeta("s1")).toBe(m);
  });
});
