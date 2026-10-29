/** 设置页「bot」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { GuardKeywordsPanel } from "./GuardKeywordsPanel";
import { EnvPanel } from "./EnvPanel";
import type { Dispatch, JSX, SetStateAction } from "react";
import type { EnvCheck, ParamView, SettingsConfig } from "./types";

export type BotCoreSectionProps = {
  botEnabled: boolean;
  toggleBot: () => Promise<void>;
  botBusy: boolean;
  botError: string;
  pyEnabled: boolean;
  togglePy: () => Promise<void>;
  pyBusy: boolean;
  servicesEnabled: boolean;
  toggleServices: () => Promise<void>;
  servicesBusy: boolean;
  exposeNetAllowed: boolean;
  toggleExposeNet: () => Promise<void>;
  exposeNetBusy: boolean;
  uiEnabled: boolean;
  toggleUi: () => Promise<void>;
  uiBusy: boolean;
  envErr: string;
  env: EnvCheck | null;
  envBusy: boolean;
  checkEnv: () => Promise<void>;
  pyTimeoutView: ParamView | undefined;
  config: SettingsConfig;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  renderSaveButton: (cls: string) => JSX.Element;
  configBusy: boolean;
  saveConfig: (override?: SettingsConfig | ((c: SettingsConfig) => SettingsConfig), opts?: { skipReload?: boolean; }) => Promise<boolean>;
};

export function BotCoreSection({
  botEnabled,
  toggleBot,
  botBusy,
  botError,
  pyEnabled,
  togglePy,
  pyBusy,
  servicesEnabled,
  toggleServices,
  servicesBusy,
  exposeNetAllowed,
  toggleExposeNet,
  exposeNetBusy,
  uiEnabled,
  toggleUi,
  uiBusy,
  envErr,
  env,
  envBusy,
  checkEnv,
  pyTimeoutView,
  config,
  setConfig,
  renderSaveButton,
  configBusy,
  saveConfig,
}: BotCoreSectionProps) {
  return (
    <>
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          机器人设置
        </h2>

        {/* 内置机器人聊天开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              开启机器人聊天
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              在主窗口「聊天」页与大模型对话，用它管理任务
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              botEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleBot}
            disabled={botBusy}
          >
            {botBusy ? "…" : botEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {botError && (
          <p className="mt-2 text-xs text-[var(--danger)]">
            {botError}
          </p>
        )}

        {/* Python 编程开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              允许机器人执行 Python
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              文档处理/编程工具的本机执行开关；沙箱：独立临时目录 +
              限时执行 + 审计留痕，默认关闭
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              pyEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={togglePy}
            disabled={pyBusy}
          >
            {pyBusy ? "…" : pyEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {/* 受管长驻服务开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              允许机器人启动本地服务
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              起长驻进程（node MCP server / dev server / watcher），
              归机器人托管、可查可停、退出时自动收干净；
              被启动的代码会继承宿主环境变量（含其中密钥），默认关闭
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              servicesEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleServices}
            disabled={servicesBusy}
          >
            {servicesBusy ? "…" : servicesEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {/* 非 loopback 暴露开关（L1 的放开方式） */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              允许服务暴露到非 loopback
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              默认只许受管服务监听 127.0.0.1 / ::1。打开后允许绑
              0.0.0.0 / :: —— 同局域网的机器能连上它
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              exposeNetAllowed ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleExposeNet}
            disabled={exposeNetBusy}
          >
            {exposeNetBusy
              ? "…"
              : exposeNetAllowed
                ? "已开启"
                : "已关闭"}
          </button>
        </div>

        {/* 桌面 UI 自动化开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              允许机器人操作桌面界面
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              读取窗口控件、点击、输入文字（原生应用 / 系统弹窗 / 已登录窗口）；
              涉及发送、提交、支付、删除的点击会弹窗请用户确认，默认关闭
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              uiEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleUi}
            disabled={uiBusy}
          >
            {uiBusy ? "…" : uiEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {/* Python 默认超时：pandas 大计算 60s 偏紧；模型可用 timeoutSecs 参数临时调。
            默认值与上限取自后端参数表（tools.python_timeout_secs）——前端此前
            自己写着「60」和「300」，后端常量一改页面就撒谎。 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              Python 默认超时（秒）
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              {pyTimeoutView
                ? `留空 = ${pyTimeoutView.default} 秒；大计算可调大，上限 ${pyTimeoutView.max} 秒（超出自动钳制）`
                : "留空 = 用内置默认；大计算可调大（超出自动钳制）"}
            </p>
          </div>
          <input
            aria-label="Python 默认超时（秒）"
            value={config.pythonTimeoutSecs}
            onChange={(e) =>
              setConfig((c) => ({
                ...c,
                pythonTimeoutSecs: e.target.value,
              }))
            }
            placeholder={pyTimeoutView?.default ?? ""}
            inputMode="numeric"
            className="nm-inset shrink-0 w-24 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none text-right"
          />
        </div>
        {/* Python 超时等 setConfig 字段靠本卡保存钮落盘（bot 关闭时 model 卡按钮不可达） */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>

      {/* 本机运行环境卡（独立卡，不塞进「机器人设置」里——那卡讲的是开关，
          这卡讲的是本机装了什么，两件事不该挤在一张卡里） */}
      <EnvPanel env={env} busy={envBusy} error={envErr} onCheck={checkEnv} />


      {/* 幻觉守卫触发词表：独立卡片（自带保存按钮，与 renderSaveButton
          无关——它只管下面那张 Agent 运行参数卡） */}
      <GuardKeywordsPanel
        value={config.guardClaimKeywords}
        defaults={config.guardClaimKeywordsDefault}
        onChange={(v) =>
          setConfig((prev) => ({ ...prev, guardClaimKeywords: v }))
        }
        onSave={() => {
          void saveConfig();
        }}
        busy={configBusy}
      />
    </>
  );
}
