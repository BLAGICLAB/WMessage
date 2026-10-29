/** 聊天面板的「stream」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  流式事件订阅：把后端事件接到消息与用量上。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { unlistenSafe } from "../../lib/useTauriListen";
import type { Msg, SkillFailure, TurnStatus } from "./types";
import { DELTA_BATCH_MS } from "./constants";
import * as streamState from "./streamState";
import * as planStore from "./planStore";
import * as askStore from "./askStore";
import type { StreamMeta } from "./streamState";
import type { PlanEventPayload } from "./planStore";
import type { Dispatch, RefObject, SetStateAction } from "react";

export type ChatStreamParams = {
  exitChat: (sid: string) => void;
  loadFinalText: (sid: string, attempt?: number) => Promise<void>;
  metaFor: (sid: string | null) => StreamMeta | undefined;
  sessionIdRef: RefObject<string | null>;
  setMessages: Dispatch<SetStateAction<Msg[]>>;
  setSessionUsage: Dispatch<SetStateAction<Record<string, { input: number; output: number; cacheRead: number; cacheCreation: number; lastInput: number; lastCacheRead: number; }>>>;
  streamHandlersRef: RefObject<Record<string, (p: unknown) => void>>;
};

export function useChatStream(params: ChatStreamParams) {
  const {
    exitChat,
    loadFinalText,
    metaFor,
    sessionIdRef,
    setMessages,
    setSessionUsage,
    streamHandlersRef,
  } = params;

  useEffect(() => {
    const pending = { text: "", think: false };
    let frame: number | null = null;
    const rafOk = typeof requestAnimationFrame === "function";
    // 事件处理器登记处：实时订阅与「回放补齐」走**同一批**处理函数。
    // 两套口径迟早分家——回放出来的正文与实时出来的对不上，用户看到的是
    // 「重说了一遍」。所以这里登记一份，回放时按事件名取同一份来调。
    const handlers: Record<string, (p: unknown) => void> = {};
    const reg = <T,>(name: string, cb: (p: T) => void) => {
      handlers[name] = cb as (p: unknown) => void;
      return listen<T>(name, (e) => cb(e.payload));
    };
    // 处理器表交给组件（回放补齐按事件名取同一份）
    streamHandlersRef.current = handlers;
    const flush = () => {
      frame = null;
      const add = pending.text;
      const thinkTouched = pending.think;
      pending.text = "";
      pending.think = false;
      if (!add && !thinkTouched) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) {
          // 后端主动起的一轮（子任务收尾汇报）没有前端预置的气泡，数组末尾是
          // 用户消息。原来在这里直接 return prev，增量被静默丢掉，用户得重新
          // 加载会话才看得见这段回复——这里补一个气泡接住它。
          return [
            ...prev,
            {
              role: "assistant" as const,
              content: add ?? "",
              streaming: true,
              turnStatus: "running" as const,
              turnStartedAt: Date.now(),
            },
          ];
        }
        const copy = [...prev];
        copy[copy.length - 1] = {
          ...last,
          content: add ? last.content + add : last.content,
          thinking: thinkTouched
            ? streamState.metaOf(sessionIdRef.current ?? "")?.thinking
            : last.thinking,
        };
        return copy;
      });
    };
    const schedule = () => {
      if (frame !== null) return;
      frame = rafOk
        ? requestAnimationFrame(flush)
        : window.setTimeout(flush, DELTA_BATCH_MS);
    };
    const unlisten = reg<{
      text?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-chat-delta", (p) => {
        const sid = p?.sessionId ?? null;
        const t = p?.text;
        if (!t) return;
        // 权威副本按「回复所属会话」累加（切走再切回时占位气泡要回填这半截）
        if (
          sid !== null &&
          (streamState.isInflight(sid) || sid === sessionIdRef.current)
        ) {
          // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
          if (!streamState.noteSeq(sid, p?.seq)) return;
          const m = metaFor(sid);
          if (m) m.text = (m.text ?? "") + t;
        }
        if (sid !== sessionIdRef.current) return;
        pending.text += t;
        schedule();
      });
    const unThink = reg<{
      text?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-think-delta", (p) => {
        const sid = p?.sessionId ?? null;
        const t = p?.text;
        if (!t) return;
        // 权威副本按「回复所属会话」累加（SWITCH-1：不随视图切换丢失），
        // 仅展示走同帧合并且只进当前视图
        if (
          (sid !== null && streamState.isInflight(sid)) ||
          sid === sessionIdRef.current
        ) {
          const m = metaFor(sid);
          if (m) m.thinking = (m.thinking ?? "") + t;
        }
        if (sid !== sessionIdRef.current) return;
        pending.think = true;
        schedule();
      },
    );
    const unTool = reg<{
      id?: string;
      name?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool", (p) => {
      const sid = p?.sessionId ?? null;
      // 元数据按回复所属会话累加（SWITCH-1）；视觉更新仅当前视图
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name } = p ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = [...(m.tools ?? [])];
      if (!tools.some((x) => x.id === id)) tools.push({ id, name: name ?? "" });
      m.tools = tools;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    const unToolName = reg<{
      id?: string;
      name?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-name", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name } = p ?? {};
      if (!id || !name) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = (m.tools ?? []).map((x) =>
        x.id === id ? { ...x, name } : x,
      );
      m.tools = tools;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    const unToolDone = reg<{
      id?: string;
      name?: string;
      args?: string;
      result?: string | null;
      ms?: number | null;
      ok?: boolean;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-done", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name, args, result, ms, ok } = p ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = (m.tools ?? []).map((x) =>
        x.id === id
          ? {
              ...x,
              name: name ?? x.name,
              args,
              done: true,
              result: result ?? undefined,
              ms: ms ?? undefined,
              ok,
            }
          : x,
      );
      m.tools = tools;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    const unToolRunning = reg<{
      id?: string;
      name?: string;
      args_preview?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-running", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name, args_preview: argsPreview } = p ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      // 记本地开始时刻：工具返回后以 done 事件的 ms 为准，前端计时只在
      // 「bot-tool 已收到但还没回」这段窗口里显示，不与后端耗时重复计。
      const tools = (m.tools ?? []).map((x) =>
        x.id === id ? { ...x, name: name ?? x.name, argsPreview, startedAt: Date.now() } : x,
      );
      m.tools = tools;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    //  回合状态机（借鉴 ZCode turn view model）：running/settling 都算进行中。
    //  settling 是关键——正文已输出完但终稿自检还在跑，此时若把气泡置为
    //  「完成」，用户看到的正是一句「说完就停」的假静止。
    const unTurnStatus = reg<{
      status?: TurnStatus;
      detail?: string;
      sessionId?: string | null;
      seq?: number;
      startedAt?: number;
    }>("bot-turn-status", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const status = p?.status;
      if (!status) return;
      // 回合终态：答完的提问卡清走（提问阻塞模型轮，终态前引擎必已回落；
      // 未答的卡正常不存在，clearResolved 防御性不动它）
      if (status !== "running" && status !== "settling") {
        askStore.clearResolved(sid);
      }
      // 回合终态 = 这一轮结束。此刻还压在合并窗口里、没落地的增量属于**这一轮**：
      // 落地它就等于给一个已经收尾的回合再开一个「正在思考」的气泡——没人会来
      // 收它（终态事件已经来过了），活体指示永远转到用户以为还在跑
      // （真机 2026-10-26：点了停止仍显示「正在思考 · 1m 32s」）。
      //
      // 丢掉的只是尾巴那几百字：最终答复里全都有（正文以库/返回值为准），
      // 而那一版的正文本来就会整表替换掉流式气泡。
      if (
        status !== "running" &&
        status !== "settling" &&
        sid === sessionIdRef.current
      ) {
        pending.text = "";
        pending.think = false;
      }
      const m = metaFor(sid);
      if (m) {
        m.turnStatus = status;
        m.turnDetail = p?.detail ?? undefined;
        // 终态到来前记下开始时刻：running/settling 期间的秒表从这里起。
        // 起点取后端记的本轮起始时刻——回灌接回时「本地此刻」是补齐那一刻，
        // 拿它当起点秒表会从零开始（回复其实已经跑了一会儿）
        if (status === "running" && m.turnStartedAt == null) {
          m.turnStartedAt = p?.startedAt ?? Date.now();
        }
      }
      // 回灌接回的那一轮：终态到了要把最终正文取回来——这轮没有 invoke
      // 结果可等（发起它的那个窗口已经没了），只能自己去取库里的最终态
      if (
        status !== "running" &&
        status !== "settling" &&
        streamState.isResumed(sid)
      ) {
        streamState.clearResumed(sid);
        exitChat(sid);
        void loadFinalText(sid);
      }
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = {
          ...last,
          turnStatus: status,
          turnDetail: p?.detail ?? undefined,
          turnStartedAt:
            status === "running"
              ? (m?.turnStartedAt ?? last.turnStartedAt ?? Date.now())
              : last.turnStartedAt,
        };
        return copy;
      });
    });
    //  结构化文件变更：dispatch 在有 trace 的执行会话里 emit（全窗口）——
    // 每次落盘修改一条，按会话累积进 meta，收尾并入最终消息，作交付物区
    // 「produced」半边的来源。主聊天（无 trace）不产生事件 → 那半边为空，
    // 交付物只来自正文自报。
    const unFileChanged = reg<{
      path?: string;
      kind?: "create" | "modify" | "delete";
      added?: number;
      deleted?: number;
      sessionId?: string | null;
      seq?: number;
    }>("bot-file-changed", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      if (!p?.path) return;
      const m = metaFor(sid);
      if (!m) return;
      const changes = [
        ...(m.fileChanges ?? []),
        {
          path: p.path,
          kind: p.kind ?? "modify",
          added: p.added ?? 0,
          deleted: p.deleted ?? 0,
        },
      ];
      m.fileChanges = changes;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, fileChanges: changes };
        return copy;
      });
    });
    // Skill 失败半成品：后端 run_skill_scheduler 返回
    // FailedButRecoverable 时 emit `bot-skill-failed` event；前端把它挂到当前流式消息上
    // 渲染 ⚠️ 折叠行，让用户看到哪步成功哪步失败 + 是否回滚。
    // 注意：失败时同一条流式消息后续还会有 LLM 兜底回复，所以不要清空 streamingMeta。
    const unSkillFailed = reg<{
      skillName?: string;
      reason?: string;
      completedSummary?: string;
      rollbackAttempted?: boolean;
      sessionId?: string | null;
      seq?: number;
    }>("bot-skill-failed", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      if (!p?.skillName) return;
      const failure: SkillFailure = {
        skillName: p.skillName,
        reason: p.reason ?? "(无原因)",
        completedSummary: p.completedSummary ?? "(无已完成步骤)",
        rollbackAttempted: !!p.rollbackAttempted,
      };
      const m = metaFor(sid);
      if (!m) return;
      m.skillFailure = failure;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, skillFailure: failure };
        return copy;
      });
    });
    //  执行计划：`update_plan` 工具整表写入会话计划表，经 `bot-plan` 事件广播。
    //  与其它流式事件同一批处理器——实时订阅与「回放补齐」共用它：回放出来的
    //  计划事件也要落到面板，否则重连后面板还停在旧状态。
    //  不走序号闸：计划是整表状态，按 updatedAt 判新旧更贴切（序号闸是给
    //  流式增量用的——内容判重做不到那里）。
    const unPlan = reg<PlanEventPayload>("bot-plan", (p) => {
      planStore.applyEvent(p);
    });
    //  会话内提问卡：ask_user 发起（bot-ask）与终态（bot-ask-resolved）。
    //  与其它流式事件同一批处理器——实时订阅与「回放补齐」共用：重开会话时
    //  回放里的问题卡要还原（含终态），否则补齐后卡就不见了。
    const unAsk = reg<{
      questionId?: string;
      question?: string;
      why?: string | null;
      options?: unknown;
      assumption?: string | null;
      sessionId?: string | null;
      seq?: number;
    }>("bot-ask", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      askStore.applyAsk(p);
    });
    const unAskResolved = reg<{
      questionId?: string;
      action?: string;
      answer?: string | null;
      sessionId?: string | null;
      seq?: number;
    }>("bot-ask-resolved", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      askStore.applyResolved(p);
    });
    //  水位条数据源：Anthropic 回合级 usage 按会话累计（当前视图 + 在途会话，
    //  与流式事件同过滤口径——切走的会话继续累计，切回可见）
    const unUsage = listen<{
      inputTokens?: number;
      outputTokens?: number;
      cacheReadTokens?: number;
      cacheCreationTokens?: number;
      sessionId?: string | null;
    }>("bot-usage-delta", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      const i = e.payload?.inputTokens ?? 0;
      const o = e.payload?.outputTokens ?? 0;
      const cr = e.payload?.cacheReadTokens ?? 0;
      const cc = e.payload?.cacheCreationTokens ?? 0;
      // 四项全空才跳过：缓存命中的回合 input/output 极小甚至为 0，只看前两项
      // 会把整段缓存量丢在账外（与 trace 弹窗的分档展示对不上）
      if (i <= 0 && o <= 0 && cr <= 0 && cc <= 0) return;
      setSessionUsage((prev) => {
        const cur = prev[sid] ?? {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheCreation: 0,
          lastInput: 0,
          lastCacheRead: 0,
        };
        return {
          ...prev,
          [sid]: {
            input: cur.input + i,
            output: cur.output + o,
            cacheRead: cur.cacheRead + cr,
            cacheCreation: cur.cacheCreation + cc,
            lastInput: i,
            lastCacheRead: cr,
          },
        };
      });
    });
    return () => {
      if (frame !== null) {
        if (rafOk) cancelAnimationFrame(frame);
        else window.clearTimeout(frame);
        frame = null;
      }
      unlistenSafe(unlisten);
      unlistenSafe(unThink);
      unlistenSafe(unTool);
      unlistenSafe(unToolName);
      unlistenSafe(unToolDone);
      unlistenSafe(unToolRunning);
      unlistenSafe(unTurnStatus);
      unlistenSafe(unFileChanged);
      unlistenSafe(unUsage);
      unlistenSafe(unSkillFailed);
      unlistenSafe(unPlan);
      unlistenSafe(unAsk);
      unlistenSafe(unAskResolved);
      streamHandlersRef.current = {};
    };
  }, [loadFinalText]);

  return {
  };
}
