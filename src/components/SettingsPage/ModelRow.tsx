// SettingsPage 子模块：模型列表紧凑行（U11，对齐截图）。
// 非编辑态：radio（设为当前）+ 模型名（mono）+ 上下文徽标 + 铅笔（编辑）+ 开关（启用）。
// 编辑态（铅笔切换）：名称 / Base URL / model id 三输入 + 删除。
// 开关 = ModelEntry.enabled（聊天 🧠 下拉只显示 enabled 模型）。

import { useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
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

export function ModelRow({
  model,
  isActive,
  apiProvider,
  onChange,
  onSelect,
  onDelete,
}: {
  model: ModelEntry;
  isActive: boolean;
  apiProvider: ApiProvider;
  onChange: (patch: Partial<ModelEntry>) => void;
  onSelect: () => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const isOn = model.enabled !== false;

  if (!editing) {
    return (
      <div
        className={`flex items-center gap-2 rounded-xl px-3 py-2 ${
          isActive ? "nm-inset" : "nm-outset"
        }`}
      >
        {/* radio：设为当前（聊天实际调用） */}
        <button
          type="button"
          role="radio"
          aria-checked={isActive}
          onClick={onSelect}
          aria-label={
            isActive ? "当前选中（点其它条目可切换）" : "点此切换为当前模型"
          }
          className={`shrink-0 h-3.5 w-3.5 rounded-full border transition-colors ${
            isActive
              ? "border-[var(--t1)] bg-[var(--t1)]"
              : "border-[var(--t4)] hover:border-[var(--t2)]"
          }`}
          title={isActive ? "当前选中（点其它条目可切换）" : "点此切换为当前模型"}
        />
        {/* 模型名（mono，截图风格） */}
        <span
          className={`min-w-0 flex-1 truncate font-mono text-xs ${
            isActive ? "text-[var(--t1)]" : "text-[var(--t3)]"
          }`}
          title={model.label || model.model}
        >
          {model.label || model.model || "（未命名）"}
        </span>
        <ContextBadge contextK={model.contextK} />
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
        <Toggle checked={isOn} onChange={(v) => onChange({ enabled: v })} />
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
      </div>
    </div>
  );
}
