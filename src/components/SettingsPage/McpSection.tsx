/** 设置页「mcp」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { SettingRow } from "./SettingRow";
import { TriangleAlert } from "lucide-react";
import type { Dispatch, JSX, SetStateAction } from "react";
import type { SettingsConfig } from "./types";

export type McpSectionProps = {
  config: SettingsConfig;
  toggleTavily: () => Promise<void>;
  configBusy: boolean;
  tavilyKeyInput: string;
  setTavilyKeyInput: Dispatch<SetStateAction<string>>;
  toggleBrave: () => Promise<void>;
  braveKeyInput: string;
  setBraveKeyInput: Dispatch<SetStateAction<string>>;
  toggleDeepseek: () => Promise<void>;
  toggleDeepseekFetch: () => Promise<void>;
  deepseekKeyInput: string;
  setDeepseekKeyInput: Dispatch<SetStateAction<string>>;
  renderSaveButton: (cls: string) => JSX.Element;
};

/** 搜索引擎开关的公共 UI：开关钮 + 状态文案 */
function EngineToggle({
  label,
  description,
  on,
  busy,
  onClick,
}: {
  label: string;
  description: string;
  on: boolean;
  busy: boolean;
  onClick: () => void;
}) {
  return (
    <SettingRow label={label} description={description}>
      <button
        className={`btn-sm-action ${on ? "nm-inset" : "nm-outset"}`}
        onClick={onClick}
        disabled={busy}
      >
        {busy ? "…" : on ? "已开启" : "已关闭"}
      </button>
    </SettingRow>
  );
}

/** 多开冲突提示文案：列出开启中的引擎（与后端 Conflict 报错同源，「与」连接） */
function conflictText(enabled: string[]): string | null {
  if (enabled.length < 2) return null;
  return `${enabled.join(" 与 ")} 只能开启一个，请关闭其中一个。`;
}

export function McpSection({
  config,
  toggleTavily,
  configBusy,
  tavilyKeyInput,
  setTavilyKeyInput,
  toggleBrave,
  braveKeyInput,
  setBraveKeyInput,
  toggleDeepseek,
  toggleDeepseekFetch,
  deepseekKeyInput,
  setDeepseekKeyInput,
  renderSaveButton,
}: McpSectionProps) {
  const conflict = conflictText(
    [
      config.deepseekEnabled ? "DeepSeek" : null,
      config.tavilyEnabled ? "Tavily" : null,
      config.braveEnabled ? "Brave" : null,
    ].filter((x): x is string => x !== null),
  );
  return (
    <>
      <div className="nm-card p-5">
        {/* Tavily 搜索 */}
        <div className="space-y-1">
          <EngineToggle
            label="Tavily 搜索"
            description="开启后 web_search 走 Tavily API（需填 key）；关闭走 Bing+百度双引擎"
            on={config.tavilyEnabled}
            busy={configBusy}
            onClick={() => void toggleTavily()}
          />
          <p className="text-xs text-[var(--t5)]">
            Tavily 搜索 API Key（可选）
          </p>
          <input
            type="password"
            value={tavilyKeyInput}
            onChange={(e) => setTavilyKeyInput(e.target.value)}
            placeholder={
              config.hasTavilyKey
                ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                : "tvly-…（开关关闭时不使用）"
            }
            className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
          />
          {config.tavilyEnabled &&
          !config.hasTavilyKey &&
          !tavilyKeyInput.trim() ? (
            <p className="text-xs text-[var(--danger)] leading-snug">
              已开启但未填 key：web_search 会报错提示，请填写 key
              后点「保存配置」，或关闭开关。
            </p>
          ) : (
            <p className="text-xs text-[var(--t6)] leading-snug">
              key 存系统凭据存储（与主 API key
              同），不再明文写进配置文件；填 key 后点「保存配置」生效；
              要停用请关闭开关（已存的 key
              不提供清除入口）。开关开启但缺 key / Tavily
              请求失败时会明确报错，不会静默走百度。
            </p>
          )}
        </div>
        {/* Brave 搜索（照搬 Tavily 模式；与 Tavily 互斥） */}
        <div className="space-y-1">
          <EngineToggle
            label="Brave 搜索"
            description="开启后 web_search 走 Brave Search API（需填 key）；关闭走 Bing+百度双引擎"
            on={config.braveEnabled}
            busy={configBusy}
            onClick={() => void toggleBrave()}
          />
          <p className="text-xs text-[var(--t5)]">
            Brave 搜索 API Key（可选）
          </p>
          <input
            type="password"
            value={braveKeyInput}
            onChange={(e) => setBraveKeyInput(e.target.value)}
            placeholder={
              config.hasBraveKey
                ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                : "BSA…（https://brave.com/search/api/ 免费获取；开关关闭时不使用）"
            }
            className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
          />
          {config.braveEnabled &&
          !config.hasBraveKey &&
          !braveKeyInput.trim() ? (
            <p className="text-xs text-[var(--danger)] leading-snug">
              已开启但未填 key：web_search 会报错提示，请填写 key
              后点「保存配置」，或关闭开关。
            </p>
          ) : (
            <p className="text-xs text-[var(--t6)] leading-snug">
              key 存系统凭据存储（与主 API key
              同），不再明文写进配置文件；填 key 后点「保存配置」生效；
              要停用请关闭开关（已存的 key 不提供清除入口）。与 Tavily
              互斥，两个开关同时开启会报错。开关开启但缺 key / Brave
              请求失败时会明确报错，不会静默走百度。
            </p>
          )}
        </div>
        {/* DeepSeek 搜索（与 Tavily/Brave 互斥；key 与「DeepSeek 抓取」共用） */}
        <div className="space-y-1">
          <EngineToggle
            label="DeepSeek 搜索"
            description="开启后 web_search 走 DeepSeek Search API（中文质量最好，每条结果带发布日期）；关闭走 Bing+百度双引擎"
            on={config.deepseekEnabled}
            busy={configBusy}
            onClick={() => void toggleDeepseek()}
          />
          <p className="text-xs text-[var(--t5)]">
            DeepSeek API Key（可选，与 DeepSeek 大模型 API 同一把平台 key）
          </p>
          <input
            type="password"
            value={deepseekKeyInput}
            onChange={(e) => setDeepseekKeyInput(e.target.value)}
            placeholder={
              config.hasDeepseekKey
                ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                : "sk-…（platform.deepseek.com 申请；开关关闭时不使用）"
            }
            className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
          />
          {config.deepseekEnabled &&
          !config.hasDeepseekKey &&
          !deepseekKeyInput.trim() ? (
            <p className="text-xs text-[var(--danger)] leading-snug">
              已开启但未填 key：web_search 会报错提示，请填写 key
              后点「保存配置」，或关闭开关。
            </p>
          ) : (
            <p className="text-xs text-[var(--t6)] leading-snug">
              key 存系统凭据存储（与主 API key
              同），不再明文写进配置文件；填 key 后点「保存配置」生效；
              要停用请关闭开关（已存的 key 不提供清除入口）。与 Tavily/Brave
              互斥，多开报错。按成功搜索计费（约 5 元/千次，搜索不到不收费），从
              DeepSeek 平台余额扣。
            </p>
          )}
        </div>
        {/* DeepSeek 抓取：手动开关（默认关），与「DeepSeek 搜索」共用同一把 key */}
        <div className="space-y-1">
          <EngineToggle
            label="DeepSeek 抓取"
            description="开启后 fetch_url 优先走 DeepSeek Fetch API（返回整页 Markdown，反爬站点更稳），失败自动回退原直连链路；默认关"
            on={config.deepseekFetchEnabled}
            busy={configBusy}
            onClick={() => void toggleDeepseekFetch()}
          />
          {config.deepseekFetchEnabled && !config.hasDeepseekKey ? (
            <p className="text-xs text-[var(--danger)] leading-snug">
              已开启但未配置 DeepSeek API Key：fetch_url
              会照常走原直连链路，请在上方「DeepSeek 搜索」的 key
              输入框填 key 后点「保存配置」，或关闭本开关。
            </p>
          ) : (
            <p className="text-xs text-[var(--t6)] leading-snug">
              与「DeepSeek 搜索」共用同一把 key；按成功抓取计费（约 5
              元/千次）。默认关闭——抓取走第三方须手动开启，开启后每次
              fetch_url 都会优先由 DeepSeek 服务端抓取（审计事件
              web.deepseek_fetch_used 留痕）。
            </p>
          )}
        </div>
        {/* 多开冲突提示：后端同样明确报错，这里提前可见 */}
        {conflict && (
          <p className="text-xs text-[var(--danger)] leading-snug">
            <TriangleAlert
              size={10}
              aria-hidden
              className="inline-block align-[-1px]"
            />{" "}
            {conflict}
          </p>
        )}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>
    </>
  );
}
