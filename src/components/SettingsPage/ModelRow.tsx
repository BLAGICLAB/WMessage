// SettingsPage 子模块：模型列表紧凑行（U11，对齐截图）。
// 非编辑态：模型名（mono）+ 上下文徽标 + 能力徽标（视觉等）+
// 插头（连接测试）+ 铅笔（编辑）+ 开关（启用）。
// 编辑态（铅笔切换）：名称 / Base URL / model id 三输入 + 视觉能力 checkbox + 删除
// + 每模型推理参数（temperature/top_p/max_tokens/system prompt，留空 = 跟随全局/默认）。
// 开关 = ModelEntry.enabled（聊天 🧠 下拉只显示 enabled 模型）。
// 厂商被禁用（vendorDisabled）时整行变淡、启用开关与连接测试禁用。

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Pencil, Plug, Trash2 } from "lucide-react";
import { formatCommandError } from "../../lib/errorHandler";
import { Toggle } from "../Toggle/Toggle";
import type { ModelEntry } from "./types";
import type { ApiProvider } from "./constants";

/** 上下文徽标：contextK=204.8 → 「204.8K」 */
function ContextBadge({ contextK }: { contextK?: number }) {
  if (!contextK) return null;
  return (
    <span className="shrink-0 rounded border border-[var(--edge)] px-1.5 py-0.5 font-mono text-[10px] leading-4 text-[var(--t5)]">
      {contextK}K
    </span>
  );
}

type TestState = "idle" | "testing" | "ok" | "fail";

export function ModelRow({
  model,
  apiProvider,
  vendorDisabled = false,
  vendorVerified = false,
  onChange,
  onDelete,
  onTested,
}: {
  model: ModelEntry;
  apiProvider: ApiProvider;
  /** 厂商总开关关闭：整行变淡、启用开关/连接测试禁用 */
  vendorDisabled?: boolean;
  /** 厂商已通过连接测试（持久化 verified_vendors）：插头常显绿色；
   *  本行刚测失败时仍显红（后端已把厂商移出名单，刷新后回到未验证态） */
  vendorVerified?: boolean;
  onChange: (patch: Partial<ModelEntry>) => void;
  onDelete: () => void;
  /** 连接测试完成后回调（后端已把可用性落盘 verified_vendors；
   *  父级用它 reload 配置刷新左栏绿点，并广播让聊天下拉同步过滤） */
  onTested?: () => void;
}) {
  const [editing, setEditing] = useState(false);
  /** 连接测试结果（本地态，不入库）：ok=绿 / fail=红，title 显示细节 */
  const [testState, setTestState] = useState<TestState>("idle");
  const [testMsg, setTestMsg] = useState("");
  const isOn = model.enabled !== false;
  // 插头颜色：本行刚测失败 > 红；已验证（持久化）或刚测通过 > 绿；否则灰
  const plugOk = testState === "ok" || (testState !== "fail" && vendorVerified === true);

  /** 插头连接测试：bot_test_connection 只判 HTTP 状态（后端 5s 超时） */
  const runTest = async () => {
    if (testState === "testing" || vendorDisabled) return;
    setTestState("testing");
    setTestMsg("");
    try {
      const r = await invoke<{ ok: boolean; status?: number; error?: string }>(
        "bot_test_connection",
        { baseUrl: model.baseUrl, apiFormat: apiProvider, vendor: model.vendor ?? null },
      );
      if (r.ok) {
        setTestState("ok");
        setTestMsg(r.status ? `连接成功（HTTP ${r.status}）` : "连接成功");
      } else {
        setTestState("fail");
        setTestMsg(r.error ?? (r.status ? `HTTP ${r.status}` : "连接失败"));
      }
    } catch (e) {
      setTestState("fail");
      setTestMsg(formatCommandError(e));
    }
    // 后端已把可用性落盘（ok 进 verified_vendors / 失败移出）：通知父级刷新
    onTested?.();
  };

  if (!editing) {
    return (
      <div
        className={`flex items-center gap-2 rounded-xl px-3 py-2 nm-outset ${
          vendorDisabled ? "opacity-50" : ""
        }`}
      >
        {/* 模型名（mono，截图风格）；当前使用模型在聊天窗口 🧠 下拉切换，此处不标 */}
        <span
          className="min-w-0 flex-1 truncate font-mono text-xs text-[var(--t2)]"
          title={model.label || model.model}
        >
          {model.label || model.model || "（未命名）"}
        </span>
        <ContextBadge contextK={model.contextK} />
        {/* 能力徽标（视觉等） */}
        {(model.capabilities ?? []).map((cap) => (
          <span key={cap} className="nm-tag shrink-0 text-[10px]">
            {cap}
          </span>
        ))}
        {/* 插头：连接测试（openai → GET /models，anthropic → GET /v1/models）；
            绿色持久化——厂商在 verified_vendors 里即常显 */}
        <button
          type="button"
          aria-label="测试连接"
          disabled={vendorDisabled || testState === "testing"}
          className={`nm-icon-btn shrink-0 ${
            testState === "fail"
              ? "text-[var(--danger)]"
              : plugOk
                ? "text-[var(--success)]"
                : "text-[var(--t5)] hover:text-[var(--t2)]"
          }`}
          title={
            testState === "testing"
              ? "连接测试中…"
              : testMsg
                ? `连接测试：${testMsg}`
                : plugOk
                  ? "连接测试已通过（改 key/URL/格式后需重新测试）"
                  : "测试连接（按该模型的 Base URL + API 格式探测 /models）"
          }
          onClick={runTest}
        >
          <Plug size={13} aria-hidden />
        </button>
        {/* 铅笔：进入编辑态 */}
        <button
          type="button"
          aria-label="编辑此模型"
          className="nm-icon-btn shrink-0 text-[var(--t5)] hover:text-[var(--t2)]"
          onClick={() => setEditing(true)}
        >
          <Pencil size={13} aria-hidden />
        </button>
        {/* 启用开关：关 = 聊天 🧠 下拉不显示（复用 Toggle 原语，自带 focus-visible） */}
        <Toggle
          checked={isOn}
          onChange={(v) => onChange({ enabled: v })}
          disabled={vendorDisabled}
        />
      </div>
    );
  }

  return (
    <div className="rounded-xl nm-inset px-3 py-2">
      <div className="flex items-center gap-2">
        <input
          value={model.label}
          onChange={(e) => onChange({ label: e.target.value })}
          placeholder="名称（DeepSeek / Kimi / Claude Sonnet…）"
          className="nm-inset min-w-0 flex-1 rounded-lg px-2.5 py-1 text-xs text-[var(--t3)] outline-none"
          autoFocus
        />
        <button
          type="button"
          className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t2)]"
          onClick={() => setEditing(false)}
        >
          完成
        </button>
      </div>
      <div className="mt-1.5 space-y-1">
        <input
          value={model.baseUrl}
          onChange={(e) => onChange({ baseUrl: e.target.value })}
          placeholder={
            apiProvider === "anthropic"
              ? "https://api.anthropic.com"
              : "https://api.deepseek.com/v1"
          }
          className="nm-inset w-full rounded-lg px-2.5 py-1 text-xs text-[var(--t3)] outline-none"
        />
        <div className="flex items-center gap-2">
          <input
            value={model.model}
            onChange={(e) => onChange({ model: e.target.value })}
            placeholder={
              apiProvider === "anthropic"
                ? "claude-sonnet-4-5"
                : "deepseek-v4-flash"
            }
            className="nm-inset min-w-0 flex-1 rounded-lg px-2.5 py-1 text-xs text-[var(--t3)] outline-none"
          />
          {/* 能力标记：目前只支持「视觉」（写 ModelEntry.capabilities） */}
          <label className="flex shrink-0 items-center gap-1 text-[11px] text-[var(--t3)]">
            <input
              type="checkbox"
              checked={(model.capabilities ?? []).includes("视觉")}
              onChange={(e) => {
                const cur = model.capabilities ?? [];
                onChange({
                  capabilities: e.target.checked
                    ? [...cur.filter((c) => c !== "视觉"), "视觉"]
                    : cur.filter((c) => c !== "视觉"),
                });
              }}
            />
            视觉能力
          </label>
          <button
            type="button"
            onClick={onDelete}
            aria-label="删除此模型"
            className="nm-icon-btn shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
            title="删除此模型"
          >
            <Trash2 size={13} aria-hidden />
          </button>
        </div>
        {/* 每模型推理参数（可选）：留空 = 跟随全局/默认，清空即移除字段。
            非受控 + onBlur 提交：数字输入中途态（"0."）不被受控回写打断 */}
        <div className="flex items-center gap-1.5">
          <input
            defaultValue={model.temperature ?? ""}
            onBlur={(e) => {
              const t = e.target.value.trim();
              if (t === "") return onChange({ temperature: undefined });
              const n = Number(t);
              if (Number.isFinite(n)) onChange({ temperature: n });
            }}
            placeholder="temperature"
            aria-label="temperature"
            inputMode="decimal"
            title="temperature（留空 = 跟随默认）"
            className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-[11px] text-[var(--t3)] outline-none"
          />
          <input
            defaultValue={model.topP ?? ""}
            onBlur={(e) => {
              const t = e.target.value.trim();
              if (t === "") return onChange({ topP: undefined });
              const n = Number(t);
              if (Number.isFinite(n)) onChange({ topP: n });
            }}
            placeholder="top_p"
            aria-label="top_p"
            inputMode="decimal"
            title="top_p（留空 = 跟随默认）"
            className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-[11px] text-[var(--t3)] outline-none"
          />
          <input
            defaultValue={model.maxTokens ?? ""}
            onBlur={(e) => {
              const t = e.target.value.trim();
              if (t === "") return onChange({ maxTokens: undefined });
              const n = parseInt(t, 10);
              if (Number.isFinite(n)) onChange({ maxTokens: n });
            }}
            placeholder="max_tokens"
            aria-label="max_tokens"
            inputMode="numeric"
            title="max_tokens（留空 = 跟随默认）"
            className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-[11px] text-[var(--t3)] outline-none"
          />
        </div>
        <input
          defaultValue={model.systemPrompt ?? ""}
          onBlur={(e) => {
            const t = e.target.value;
            onChange({ systemPrompt: t === "" ? undefined : t });
          }}
          placeholder="system prompt（留空 = 跟随默认）"
          aria-label="system prompt"
          className="nm-inset w-full rounded-lg px-2 py-1 text-[11px] text-[var(--t3)] outline-none"
        />
      </div>
    </div>
  );
}
