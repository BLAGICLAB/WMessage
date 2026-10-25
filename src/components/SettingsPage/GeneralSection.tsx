/** 设置页「general」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import type { ThemeSetting } from "../../theme";
import { BubbleStyleCard } from "./BubbleStyleCard";
import { ProfileRow } from "./ProfileRow";
import { UI_FONT_SIZE_OPTIONS } from "./constants";
import { LucideIcon, Monitor, Moon, Sun } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { SettingsConfig } from "./types";

export type GeneralSectionProps = {
  theme: ThemeSetting;
  onThemeChange: (t: ThemeSetting) => void;
  config: SettingsConfig;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  saveConfig: (override?: SettingsConfig | ((c: SettingsConfig) => SettingsConfig), opts?: { skipReload?: boolean; }) => Promise<boolean>;
  autostartError: string;
  autostartEnabled: boolean;
  toggleAutostart: () => Promise<void>;
  autostartBusy: boolean;
};

export function GeneralSection({
  theme,
  onThemeChange,
  config,
  setConfig,
  saveConfig,
  autostartError,
  autostartEnabled,
  toggleAutostart,
  autostartBusy,
}: GeneralSectionProps) {
  return (
    <>
      {/* 个人资料 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          个人资料
        </h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          任务卡上的归属头像：设了定时 → 一直机器人头像；🤖
          交给机器人执行中 → 机器人头像；执行完且无定时 →
          用户头像。悬停头像显示姓名。
        </p>
        <div className="mt-4 space-y-4">
          <ProfileRow kind="user" label="用户" defaultName="我" />
          <ProfileRow kind="bot" label="机器人" defaultName="机器人" />
        </div>
      </div>

      {/* 基础设置：外观 + 开机自启动 wmessage */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          基础设置
        </h2>
        <div className="mt-4 space-y-3">
          {/* 外观（原「深浅色模式」改名：主题是外观的一部分，名字更准确） */}
          <div>
            <p className="text-sm font-medium text-[var(--t2)]">外观</p>
            <div className="mt-3 flex gap-1">
              {(
                [
                  { value: "light", label: "浅色", icon: Sun },
                  { value: "dark", label: "深色", icon: Moon },
                  { value: "system", label: "跟随系统", icon: Monitor },
                ] as {
                  value: ThemeSetting;
                  label: string;
                  icon: LucideIcon;
                }[]
              ).map(({ value, label, icon: Icon }) => (
                <button
                  key={value}
                  className={`px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                    theme === value ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={() => onThemeChange(value)}
                >
                  <Icon
                    size={13}
                    aria-hidden
                    className="inline-block align-[-2px]"
                  />{" "}
                  {label}
                </button>
              ))}
            </div>
            {/* 字体大小：四档 small/standard/large/xlarge
        默认 small（“目前字号为小”）。走 data-attr 全局套用，
        视觉缩放在 main.css 里。点选即生效 + 即时落盘（与外观一致，
        不用再点「保存配置」）。 */}
            <p className="mt-3 text-sm font-medium text-[var(--t2)]">
              字体大小
            </p>
            <div className="mt-2 flex gap-1">
              {UI_FONT_SIZE_OPTIONS.map((o) => (
                <button
                  key={o.value}
                  className={`flex-1 px-2 py-1.5 text-xs ${
                    config.uiFontSize === o.value
                      ? "nm-inset"
                      : "nm-outset"
                  } text-[var(--t3)]`}
                  onClick={() => {
                    const v = o.value;
                    setConfig((c) => ({ ...c, uiFontSize: v }));
                    // 点选即生效：data-attr 预览 + saveConfig 落盘（functional override
                    // 拿最新 config，skipReload 避免末尾重拉用磁盘上可能的旧 uiFontSize
                    // 覆盖刚点的字段）
                    document.documentElement.dataset.fontSize = v;
                    saveConfig((c) => ({ ...c, uiFontSize: v }), {
                      skipReload: true,
                    });
                  }}
                >
                  {o.label}
                </button>
              ))}
            </div>
          </div>
          {/* 开机自动启动 wmessage：走 tauri-plugin-autostart，
      走现成的 plugin:autostart|enable / disable / is_enabled 命令。
      点击即落盘：macOS 写 LaunchAgent plist / Windows 写注册表 Run / Linux 写 .desktop。 */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                开机自动启动 wmessage
              </p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                登录系统时自动拉起 wmessage，开关改完即生效
              </p>
              {autostartError && (
                <p className="mt-1 text-xs text-[var(--danger)]">
                  {autostartError}
                </p>
              )}
            </div>
            <button
              className={`btn-sm-action ${
                autostartEnabled ? "nm-inset" : "nm-outset"
              }`}
              onClick={toggleAutostart}
              disabled={autostartBusy}
            >
              {autostartBusy
                ? "…"
                : autostartEnabled
                  ? "已开启"
                  : "已关闭"}
            </button>
          </div>
        </div>
      </div>

      {/* 聊天气泡外观：用户气泡颜色 + 机器人气泡材质（纯前端偏好，即点即生效） */}
      <BubbleStyleCard />
    </>
  );
}
