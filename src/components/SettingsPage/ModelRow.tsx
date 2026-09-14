// SettingsPage 子模块：单条大模型条目（radio + label + baseUrl + model + 删除）。

import type { ModelEntry } from "./types";
import type { ApiProvider } from "./constants";

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
  return (
    <div
      className={`p-2.5 rounded-xl ${
        isActive ? "nm-inset" : "nm-outset"
      }`}
    >
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onSelect}
          className={`shrink-0 w-5 h-5 rounded-full flex items-center justify-center text-[10px] transition-colors ${
            isActive
              ? "bg-[var(--accent)] text-white"
              : "border border-[var(--edge)] text-transparent hover:border-[var(--t4)]"
          }`}
          title={isActive ? "当前选中（点其它条目可切换）" : "点此切换为当前模型"}
        >
          ●
        </button>
        <input
          value={model.label}
          onChange={(e) => onChange({ label: e.target.value })}
          placeholder="名称（DeepSeek / Kimi / Claude Sonnet…）"
          className="nm-inset flex-1 min-w-0 rounded-lg px-2.5 py-1 text-xs text-[var(--t3)] outline-none"
        />
        <button
          type="button"
          onClick={onDelete}
          className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--danger)] px-1"
          title="删除此模型"
        >
          🗑
        </button>
      </div>
      <div className="mt-1.5 ml-7 space-y-1">
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
        <input
          value={model.model}
          onChange={(e) => onChange({ model: e.target.value })}
          placeholder={
            apiProvider === "anthropic" ? "claude-sonnet-4-5" : "deepseek-v4-flash"
          }
          className="nm-inset w-full rounded-lg px-2.5 py-1 text-xs text-[var(--t3)] outline-none"
        />
      </div>
    </div>
  );
}
