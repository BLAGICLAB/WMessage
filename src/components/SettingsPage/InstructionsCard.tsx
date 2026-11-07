// SettingsPage 子模块：用户指令文件卡（memory 分区）。
//
// instructions.md 是用户手工维护的确定性指令层：每回合原样进 system，
// 与 DB 语义记忆体（自动沉淀、检索注入）互补——那边是模型自己记的，
// 这边是用户说了算的。保存即落盘，下一回合生效；超 16000 字后端拒收，
// 前端把保存钮禁掉并显字数，别让用户点完保存才见错。

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatCommandError } from "../../lib/errorHandler";

type InstructionsView = { content: string; chars: number; maxChars: number };

export function InstructionsCard() {
  // null = 载入中（区分「空文件」与「还没读到」）
  const [content, setContent] = useState<string | null>(null);
  const [maxChars, setMaxChars] = useState(16000);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const savedRef = useRef("");

  useEffect(() => {
    let alive = true;
    invoke<InstructionsView>("instructions_get")
      .then((v) => {
        if (!alive) return;
        setContent(v.content);
        setMaxChars(v.maxChars);
        savedRef.current = v.content;
      })
      .catch((e) => {
        if (alive) setMsg(`载入失败：${formatCommandError(e)}`);
      });
    return () => {
      alive = false;
    };
  }, []);

  // 按码点计数，与后端 chars().count() 同口径（代理字符算 1）
  const chars = [...(content ?? "")].length;
  const overLimit = chars > maxChars;
  const dirty = content !== null && content !== savedRef.current;

  async function save() {
    if (content === null) return;
    setBusy(true);
    try {
      const v = await invoke<InstructionsView>("instructions_set", { content });
      savedRef.current = v.content;
      setContent(v.content);
      setMsg("已保存，下一回合生效");
    } catch (e) {
      setMsg(`保存失败：${formatCommandError(e)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">指令文件</h2>
      <p className="mt-1 text-xs leading-relaxed text-[var(--t5)]">
        你手工维护的长期指令：这里的内容会随每次对话注入系统提示词——写
        回复风格、称呼习惯、固定约束这类「每次都要遵守」的事。与下方的自动
        记忆互补：记忆是模型自己沉淀的，这里是你明说了算的。保存后下一回合
        生效。
      </p>
      {content === null ? (
        <p className="mt-4 text-xs text-[var(--t4)]">载入中…</p>
      ) : (
        <>
          <textarea
            aria-label="指令文件内容"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            rows={10}
            spellCheck={false}
            className="nm-inset mt-4 w-full rounded-[var(--r-md)] px-3 py-2 text-sm leading-relaxed text-[var(--t1)] outline-none"
          />
          <div className="mt-1.5 flex items-center justify-between gap-2">
            <p className={`text-xs ${overLimit ? "text-[var(--t1)]" : "text-[var(--t5)]"}`}>
              {overLimit
                ? `⚠️ 超出上限 ${chars}/${maxChars} 字，请精简后再保存`
                : `${chars}/${maxChars} 字`}
            </p>
            {dirty && <p className="text-xs text-[var(--t4)]">未保存</p>}
          </div>
          <div className="mt-3 flex items-center gap-2">
            <button
              type="button"
              onClick={save}
              disabled={busy || overLimit || !dirty}
              className="rounded-[var(--r-md)] bg-[var(--brand)] px-4 py-1.5 text-sm font-medium text-white disabled:opacity-50"
            >
              {busy ? "保存中…" : "保存"}
            </button>
            <span className="text-xs text-[var(--t5)]">保存后下一回合生效</span>
          </div>
        </>
      )}
      {msg && <p className="mt-2 text-xs text-[var(--t4)]">{msg}</p>}
    </div>
  );
}
