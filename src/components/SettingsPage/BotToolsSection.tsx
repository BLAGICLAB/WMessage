/** 设置页「bot」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { Check } from "lucide-react";
import type { Dispatch, JSX, SetStateAction } from "react";
import type { ApiStatus, SettingsConfig } from "./types";

export type BotToolsSectionProps = {
  config: SettingsConfig;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  configBusy: boolean;
  status: ApiStatus;
  toggle: () => Promise<void>;
  busy: boolean;
  error: string;
  copyToken: () => Promise<void>;
  copied: boolean;
  rotateToken: () => Promise<void>;
  renderSaveButton: (cls: string) => JSX.Element;
};

export function BotToolsSection({
  config,
  setConfig,
  configBusy,
  status,
  toggle,
  busy,
  error,
  copyToken,
  copied,
  rotateToken,
  renderSaveButton,
}: BotToolsSectionProps) {
  return (
    <>
      <div className="nm-card p-5">
        {/*  bypass_llm_on_pre_step_hit 开关：技能路由新链路 / 旧链路回退闸 */}
        <div className="space-y-1">
          <p className="text-xs text-[var(--t5)]">智能技能路由</p>
          <button
            className={`shrink-0 min-w-[160px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              config.bypassLlmOnPreStepHit ? "nm-inset" : "nm-outset"
            }`}
            onClick={() =>
              setConfig((c) => ({
                ...c,
                bypassLlmOnPreStepHit: !c.bypassLlmOnPreStepHit,
              }))
            }
            disabled={configBusy}
          >
            {config.bypassLlmOnPreStepHit
              ? "已开启（推荐）"
              : "已关闭（回退旧链路）"}
          </button>
          <p className="text-xs text-[var(--t6)] leading-snug">
            开启（推荐）：命中技能时 auto 模式走 DSL 调度器、interactive
            模式由 LLM 驱动技能步骤。
            <br />
            关闭：退回旧链路，由主 LLM
            自由选择技能——仅当新路由行为异常时紧急回退用。
          </p>
        </div>

        {/* 外部机器人 API 开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              开启外部机器人 API 接口
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              {status.enabled
                ? `运行中 · http://127.0.0.1:${status.port}`
                : "默认关闭，开启后仅监听本机 127.0.0.1"}
            </p>
          </div>
          <button
            className={`btn-sm-action ${
              status.enabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggle}
            disabled={busy}
          >
            {busy ? "…" : status.enabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {error && (
          <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>
        )}

        {/* token 展示 + 复制 */}
        {status.enabled && (
          <>
            <div className="mt-5">
              <p className="text-xs text-[var(--t4)]">
                访问令牌（Bearer Token）
              </p>
              <div className="mt-1.5 flex items-center gap-2">
                <input
                  readOnly
                  value={status.token}
                  className="nm-inset flex-1 min-w-0 rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                  onFocus={(e) => e.currentTarget.select()}
                />
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
                  onClick={copyToken}
                >
                  {copied ? (
                    <>
                      <Check size={12} aria-hidden /> 已复制
                    </>
                  ) : (
                    "复制"
                  )}
                </button>
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--danger)]"
                  onClick={rotateToken}
                  disabled={busy}
                >
                  重新生成
                </button>
              </div>
            </div>

            {/* 接口说明 */}
            <div className="mt-5 space-y-1 text-xs text-[var(--t4)]">
              <p className="font-medium text-[var(--t3)]">
                接口（均需 Authorization: Bearer &lt;token&gt;）
              </p>
              <p>
                GET&nbsp;&nbsp;/api/health —
                健康检查（免鉴权，仅服务状态）
              </p>
              <p>
                GET&nbsp;&nbsp;/api/tasks —
                活跃任务（?status=todo|doing|done 筛选；?trash=1
                回收站；?archived=1 归档；?all=1 全量）
              </p>
              <p>GET&nbsp;&nbsp;/api/tasks/:id — 单条任务</p>
              <p>
                POST&nbsp;/api/tasks — 新建任务（title
                必填，note/status/filePath/fileIsDir/due/tags 可选）
              </p>
              <p>
                PUT&nbsp;&nbsp;/api/tasks/:id —
                更新（title/note/status/filePath/fileIsDir/due/tags/archived/deleted）
              </p>
              <p>DELETE&nbsp;/api/tasks/:id — 软删进回收站（幂等）</p>
              <p>
                GET&nbsp;&nbsp;/api/events — SSE 实时推送（?since=事件id
                断线重放）
              </p>
              <p className="mt-1 text-[var(--t5)]">
                开关状态自动记忆：退出时开启，下次启动自动恢复
              </p>
            </div>
          </>
        )}
        {/* 技能路由/外部 API 段的保存入口 */}
        {renderSaveButton("mt-4 flex justify-end")}
      </div>
    </>
  );
}
