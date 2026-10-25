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
  renderSaveButton: (cls: string) => JSX.Element;
};

export function McpSection({
  config,
  toggleTavily,
  configBusy,
  tavilyKeyInput,
  setTavilyKeyInput,
  toggleBrave,
  braveKeyInput,
  setBraveKeyInput,
  renderSaveButton,
}: McpSectionProps) {
  return (
    <>
      <div className="nm-card p-5">
        {/* Tavily 搜索 */}
        <div className="space-y-1">
          <SettingRow
            label="Tavily 搜索"
            description="开启后 web_search 走 Tavily API（需填 key）；关闭走 Bing+百度双引擎"
          >
            <button
              className={`btn-sm-action ${
                config.tavilyEnabled ? "nm-inset" : "nm-outset"
              }`}
              onClick={toggleTavily}
              disabled={configBusy}
            >
              {configBusy
                ? "…"
                : config.tavilyEnabled
                  ? "已开启"
                  : "已关闭"}
            </button>
          </SettingRow>
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
          <SettingRow
            label="Brave 搜索"
            description="开启后 web_search 走 Brave Search API（需填 key）；关闭走 Bing+百度双引擎"
          >
            <button
              className={`btn-sm-action ${
                config.braveEnabled ? "nm-inset" : "nm-outset"
              }`}
              onClick={toggleBrave}
              disabled={configBusy}
            >
              {configBusy
                ? "…"
                : config.braveEnabled
                  ? "已开启"
                  : "已关闭"}
            </button>
          </SettingRow>
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
        {/* Tavily/Brave 双开冲突提示：后端同样明确报错，这里提前可见 */}
        {config.tavilyEnabled && config.braveEnabled && (
          <p className="text-xs text-[var(--danger)] leading-snug">
            <TriangleAlert
              size={10}
              aria-hidden
              className="inline-block align-[-1px]"
            />{" "}
            Tavily 与 Brave 只能开启一个，请关闭其中一个。
          </p>
        )}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>
    </>
  );
}
