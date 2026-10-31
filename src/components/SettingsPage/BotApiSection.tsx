/** 设置页「bot」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import type { Dispatch, JSX, SetStateAction } from "react";
import type { SettingsConfig } from "./types";

export type BotApiSectionProps = {
  config: SettingsConfig;
  setPermMode: (mode: "strict" | "ask" | "auto" | "yolo") => Promise<void>;
  configBusy: boolean;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  renderSaveButton: (cls: string) => JSX.Element;
};

export function BotApiSection({
  config,
  setPermMode,
  configBusy,
  setConfig,
  renderSaveButton,
}: BotApiSectionProps) {
  return (
    <>
      <div className="nm-card p-5">
        {/* 授权模式（Kimi CLI 风格执行前授权） */}
        <div className="space-y-1">
          <p className="text-sm font-medium text-[var(--t2)]">
            授权模式
          </p>
          <div className="flex gap-2">
            {(
              [
                ["strict", "严格白名单"],
                ["ask", "询问后放行"],
                ["auto", "白名单内自动"],
                ["yolo", "yolo 全放行"],
              ] as const
            ).map(([mode, label]) => (
              <button
                key={mode}
                className={`flex-1 px-2 py-1.5 text-xs ${
                  config.permMode === mode ? "nm-inset" : "nm-outset"
                } ${mode === "yolo" ? "text-[var(--danger)]" : "text-[var(--t3)]"}`}
                onClick={() => setPermMode(mode)}
                disabled={configBusy}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="text-xs text-[var(--t6)] leading-snug">
            {config.permMode === "strict" &&
              "白名单外的文件访问一律拒绝。"}
            {config.permMode === "ask" &&
              "白名单内的文件直接读；白名单外弹窗请你授权（允许一次 / 始终允许该目录 / 拒绝）。"}
            {config.permMode === "auto" &&
              "白名单内的文件操作全自动（含覆盖写免确认）——定时/工作流无人值守写文件不再失败；白名单外仍弹窗授权。"}
            {config.permMode === "yolo" &&
              "⚠️ 不弹任何授权：机器人可读本机任意文件，且 Python 编程免开关直接执行（以本机用户权限，可联网）。仅在你完全信任所用模型时开启。"}
          </p>
        </div>
        {/* per-tool 权限规则表（deny/ask/allow 首中即停；无命中走授权模式） */}
        <div className="space-y-1">
          <div className="flex items-center justify-between">
            <p className="text-xs text-[var(--t5)]">
              工具权限规则（按工具覆盖授权模式）
            </p>
            <button
              className="nm-btn px-2 py-0.5 text-xs text-[var(--t3)]"
              onClick={() =>
                setConfig((c) => ({
                  ...c,
                  toolRules: [
                    ...c.toolRules,
                    { tool: "", action: "deny" as const },
                  ],
                }))
              }
            >
              + 添加规则
            </button>
          </div>
          {config.toolRules.length === 0 ? (
            <p className="text-xs text-[var(--t6)]">
              例：deny run_python 禁用编程工具；ask edit_file
              每次编辑都确认。规则优先于上方授权模式。
            </p>
          ) : (
            <div className="space-y-1">
              {config.toolRules.map((r, i) => (
                <div key={i} className="flex items-center gap-1.5">
                  {/* oxlint-disable-next-line react/no-array-index-key */}
                  <input
                    value={r.tool}
                    placeholder="工具名，如 run_python"
                    onChange={(e) =>
                      setConfig((c) => ({
                        ...c,
                        toolRules: c.toolRules.map((x, j) =>
                          j === i ? { ...x, tool: e.target.value } : x,
                        ),
                      }))
                    }
                    className="min-w-0 flex-1 rounded-lg nm-inset px-2 py-1 font-mono text-xs text-[var(--t2)] outline-none"
                  />
                  <select
                    aria-label={`规则动作 ${r.tool || i + 1}`}
                    value={r.action}
                    onChange={(e) =>
                      setConfig((c) => ({
                        ...c,
                        toolRules: c.toolRules.map((x, j) =>
                          j === i
                            ? {
                                ...x,
                                action: e.target.value as
                                  "allow" | "ask" | "deny",
                              }
                            : x,
                        ),
                      }))
                    }
                    className="shrink-0 rounded-lg nm-inset px-1.5 py-1 text-xs text-[var(--t3)]"
                  >
                    <option value="allow">allow 放行</option>
                    <option value="ask">ask 确认</option>
                    <option value="deny">deny 禁用</option>
                  </select>
                  <button
                    aria-label={`删除规则 ${r.tool || i + 1}`}
                    className="shrink-0 px-1 text-sm text-[var(--t5)] hover:text-[var(--danger)]"
                    onClick={() =>
                      setConfig((c) => ({
                        ...c,
                        toolRules: c.toolRules.filter(
                          (_, j) => j !== i,
                        ),
                      }))
                    }
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
        {/* 本地文件工具白名单（read_text_file/grep_files/list_files；
        追加语义：在内置默认之上追加放行） */}
        <div className="space-y-1">
          <p className="text-xs text-[var(--t5)]">
            文件工具白名单目录（一行一个绝对路径）
          </p>
          <textarea
            value={config.allowedDirs}
            onChange={(e) =>
              setConfig((c) => ({ ...c, allowedDirs: e.target.value }))
            }
            placeholder={
              "在内置默认之上追加：~/Desktop、~/Downloads、~/Documents + 任务卡绑定文件夹始终放行"
            }
            rows={3}
            className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none resize-y"
          />
          <p className="text-xs text-[var(--t6)] leading-snug">
            白名单内静默放行；白名单外按授权模式处理（见上）。授权弹窗点「始终允许该目录」会自动追加到这里。
          </p>
        </div>
        {/* 目的地白名单（http_request 等联网工具的用户信任域）：
        命中静默放行；白名单外弹窗按次授权（批准仅本会话内有效）。
        模型 API 与 MCP 端点的域名天然在信任域，不必在此登记 */}
        <div className="space-y-1">
          <p className="text-xs text-[var(--t5)]">
            联网工具目的地白名单（一行一个域名，支持 *.example.com）
          </p>
          <textarea
            value={config.allowedDomains}
            onChange={(e) =>
              setConfig((c) => ({ ...c, allowedDomains: e.target.value }))
            }
            placeholder={"api.example.com\n*.githubusercontent.com\n空 = 无白名单，白名单外每次弹窗授权"}
            rows={3}
            className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none resize-y"
          />
          <p className="text-xs text-[var(--t6)] leading-snug">
            只按域名判断（不看内容）：*.example.com 是域名级信任，同域下若有恶意内容仍可经它转发——别把不认识的泛域名加进来。
          </p>
        </div>
        {/* 白名单 textarea 等 setConfig 字段靠本卡保存钮落盘 */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>
    </>
  );
}
