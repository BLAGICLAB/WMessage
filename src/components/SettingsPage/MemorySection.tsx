/** 设置页「memory」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { MemoryPanel } from "./MemoryPanel";
import { SettingRow } from "./SettingRow";
import { AUTO_EXTRACT_MODES } from "./constants";
import type { JSX } from "react";
import type { AutoExtractMode } from "./constants";
import type { SettingsConfig } from "./types";

export type MemorySectionProps = {
  config: SettingsConfig;
  setConsolidation: (patch: Partial<{ enabled: boolean; interval: string; }>) => Promise<void>;
  configBusy: boolean;
  consolidateBusy: boolean;
  consolidateNow: () => Promise<void>;
  consolidateMsg: string;
  renderMemoryToggle: (label: string, description: string, field: "injectionEnabled" | "autoWriteEnabled") => JSX.Element;
  extractIntervalText: string | null;
  setMemoryControl: (patch: Partial<{ injectionEnabled: boolean; autoWriteEnabled: boolean; autoExtract: AutoExtractMode; }>) => Promise<void>;
};

export function MemorySection({
  config,
  setConsolidation,
  configBusy,
  consolidateBusy,
  consolidateNow,
  consolidateMsg,
  renderMemoryToggle,
  extractIntervalText,
  setMemoryControl,
}: MemorySectionProps) {
  return (
    <>
      <div className="nm-card p-5">
        {/* 定时记忆整理 */}
        <div className="space-y-2">
          <SettingRow
            label="记忆整理"
            description="定期用大模型对近期记忆做一轮反思：合并相关条目、裁决矛盾、提炼规律（需要已配置大模型 API）"
          >
            <button
              className={`btn-sm-action ${
                config.memoryConsolidation.enabled
                  ? "nm-inset"
                  : "nm-outset"
              }`}
              onClick={() =>
                setConsolidation({
                  enabled: !config.memoryConsolidation.enabled,
                })
              }
              disabled={configBusy}
            >
              {config.memoryConsolidation.enabled ? "已开启" : "已关闭"}
            </button>
          </SettingRow>
          {config.memoryConsolidation.enabled && (
            <>
              <div className="flex gap-2">
                {(
                  [
                    ["off", "关闭"],
                    // 键是后端协议值，标签只是它的中文读法——「12」不是
                    // 一个可配置数值，后端改协议时这里跟着改键即可
                    ["12h", "每 12 小时"],
                    ["daily", "每天"],
                    ["weekly", "每周"],
                  ] as const
                ).map(([iv, label]) => (
                  <button
                    key={iv}
                    className={`flex-1 px-2 py-1.5 text-xs text-[var(--t3)] ${
                      config.memoryConsolidation.interval === iv
                        ? "nm-inset"
                        : "nm-outset"
                    }`}
                    onClick={() => setConsolidation({ interval: iv })}
                    disabled={configBusy}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <div className="flex items-center justify-between gap-4">
                <p className="text-xs text-[var(--t5)]">
                  上次整理：
                  {config.memoryConsolidation.lastRunAt
                    ? new Date(
                        config.memoryConsolidation.lastRunAt,
                      ).toLocaleString("zh-CN", {
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                      })
                    : "从未"}
                </p>
                <button
                  className={`btn-sm-action ${
                    consolidateBusy ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={consolidateNow}
                  disabled={consolidateBusy}
                >
                  {consolidateBusy ? "整理中…" : "立即整理"}
                </button>
              </div>
              {consolidateMsg && (
                <p className="text-xs text-[var(--t4)]">
                  {consolidateMsg}
                </p>
              )}
            </>
          )}
        </div>
      </div>
      {/*  记忆可控开关：注入总闸 + 模型主动记忆门禁（点档即时落盘） */}
      <div className="nm-card p-5">
        <div className="space-y-3">
          <p className="text-sm font-medium text-[var(--t2)]">
            记忆权限
          </p>
          {renderMemoryToggle(
            "聊天注入记忆",
            "关闭后记忆仍会保留，但不再随对话发给模型（隐私总闸）",
            "injectionEnabled",
          )}
          {renderMemoryToggle(
            "模型自动记忆",
            "关闭后模型调用「记住 / 记教训」工具只会收到关闭提示，不写入",
            "autoWriteEnabled",
          )}
          {/* 自动记忆抽取三档：会话收尾低频触发；总闸（模型自动记忆）关闭时不跑，
      三档随总闸禁用（radiogroup/radio 与厂商开关的 switch 同一可访问标准） */}
          <div className="space-y-1.5">
            <p className="text-xs text-[var(--t3)]">自动记忆抽取</p>
            <p className="text-xs text-[var(--t5)]">
              每次聊天结束后低频
              {extractIntervalText
                ? `（≥${extractIntervalText}一次）`
                : "地"}
              用大模型从对话里提取值得长期记住的偏好与事实；
              需「模型自动记忆」开启；「需确认」档会先存入下方待确认列表
            </p>
            <div
              className="flex gap-2"
              role="radiogroup"
              aria-label="自动记忆抽取"
            >
              {AUTO_EXTRACT_MODES.map(({ value, label, aria }) => {
                const selected =
                  config.memoryControl.autoExtract === value;
                const disabled =
                  configBusy || !config.memoryControl.autoWriteEnabled;
                return (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    aria-label={aria}
                    className={`flex-1 px-2 py-1.5 text-xs text-[var(--t3)] ${
                      selected ? "nm-inset" : "nm-outset"
                    } ${disabled ? "opacity-50" : ""}`}
                    onClick={() =>
                      setMemoryControl({ autoExtract: value })
                    }
                    disabled={disabled}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </div>
      {/* 记忆库管理面板：列表/搜索/编辑/删除 + 统计与嵌入引擎状态 */}
      <MemoryPanel />
    </>
  );
}
