// SettingsPage 子模块：受管长驻服务面板。
//
// 数据流：services_list 读宿主托管的服务全景（状态 / pid / 运行时长 / 资源占用 /
// 输出尾部），services_stop 停一个服务（后端走 SIGINT→SIGTERM→SIGKILL 阶梯）。
// 面板只做「看 + 停」：起服务是机器人的活，用户在这里看着它、随时叫停。
// 资源告警（RSS / CPU）只出声不自动杀——杀正在构建的 dev server 比让它跑飞更糟。

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Server, Square } from "lucide-react";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { EmptyState } from "../EmptyState";

/** 服务状态（后端 ServiceState 的小写形态） */
type ServiceState = "starting" | "running" | "stopping" | "exited" | "failed";

/** 一个受管服务的展示视图（后端 services_snapshot_json 的单条） */
type ManagedServiceView = {
  serviceId: string;
  name: string;
  command: string;
  args: string[];
  cwd: string;
  pid: number;
  state: ServiceState;
  startedAt: number;
  uptimeSecs: number;
  exitCode: number | null;
  sessionId: string | null;
  readySnapshot?: string;
  /** 整棵进程树合计的常驻内存（MB），没采到时没有这个字段 */
  rssMb?: number;
  /** 整棵进程树合计的 CPU 占用率（百分比） */
  cpuPct?: number;
  /** 告警项（rss_over_limit / cpu_sustained_high） */
  warnings: string[];
  stdoutTail?: string[];
  stderrTail?: string[];
};

/** 服务全景（后端 services_snapshot_json） */
type ServicesSnapshot = {
  services: ManagedServiceView[];
  totalRssMb: number;
  warnings: string[];
  sampledAt: number | null;
};

/** 状态 → 中文 + 徽章色 */
const STATE_LABEL: Record<ServiceState, { text: string; cls: string }> = {
  starting: { text: "启动中", cls: "text-blue-600 bg-blue-50" },
  running: { text: "运行中", cls: "text-emerald-600 bg-emerald-50" },
  stopping: { text: "停止中", cls: "text-amber-600 bg-amber-50" },
  exited: { text: "已退出", cls: "text-[var(--t5)] bg-[var(--inset-bg)]" },
  failed: { text: "启动失败", cls: "text-red-600 bg-red-50" },
};

/** 告警项 → 中文 */
const WARNING_LABEL: Record<string, string> = {
  rss_over_limit: "内存超阈值（>512MB）",
  cpu_sustained_high: "CPU 持续跑满（>95% 持续 5 分钟）",
  total_rss_over_limit: "全部服务合计内存超阈值（>1GB）",
};

/** 运行时长 → 人类可读 */
function formatUptime(secs: number): string {
  if (secs < 60) return `${secs} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分 ${secs % 60} 秒`;
  const h = Math.floor(secs / 3600);
  return `${h} 小时 ${Math.floor((secs % 3600) / 60)} 分`;
}

/** 受管服务面板：看状态与资源占用，随时停掉不用的服务 */
export function ServicesPanel() {
  const [snapshot, setSnapshot] = useState<ServicesSnapshot | null>(null);
  const [busyId, setBusyId] = useState("");
  const [error, setError] = useState("");

  const refresh = async () => {
    try {
      setSnapshot(await invoke<ServicesSnapshot>("services_list"));
    } catch (e) {
      handleCommandError(e, "services_list", { silent: true });
      setError(formatCommandError(e));
    }
  };

  useEffect(() => {
    // 挂载即拉一次（外部数据同步既有模式）
    // oxlint-disable-next-line react/set-state-in-effect
    refresh();
    // 服务在跑时状态一直在变（pid / 运行时长 / 资源占用）→ 定时刷新
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, []);

  const stop = async (id: string) => {
    if (busyId) return;
    setBusyId(id);
    setError("");
    try {
      await invoke("services_stop", { serviceId: id });
      await refresh();
    } catch (e) {
      handleCommandError(e, "services_stop", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusyId("");
    }
  };

  const services = snapshot?.services ?? [];
  const active = services.filter(
    (s) => s.state === "running" || s.state === "starting",
  );

  return (
    <div className="nm-card p-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-[var(--t1)]">受管服务</h2>
          <p className="mt-1 text-xs text-[var(--t5)]">
            机器人起的长驻进程（node MCP server / dev server / watcher）归宿主托管：
            可查可停，退出时自动收干净。资源超阈值只告警不自动杀
          </p>
        </div>
        <button
          className="nm-btn shrink-0 px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1"
          onClick={refresh}
          aria-label="刷新服务列表"
        >
          <RefreshCw size={12} aria-hidden /> 刷新
        </button>
      </div>

      {snapshot && (
        <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[var(--t5)]">
          <span>运行中 {active.length} 个</span>
          <span>合计内存 {snapshot.totalRssMb.toFixed(0)} MB</span>
          {snapshot.warnings.map((w) => (
            <span key={w} className="text-[var(--danger)]">
              ⚠ {WARNING_LABEL[w] ?? w}
            </span>
          ))}
        </div>
      )}

      {error && <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>}

      {services.length === 0 ? (
        <EmptyState
          icon={<Server size={18} aria-hidden />}
          title="暂无受管服务"
          description="机器人用 start_service 起的服务会出现在这里"
        />
      ) : (
        <div className="mt-3 flex flex-col gap-2">
          {services.map((s) => {
            const badge = STATE_LABEL[s.state] ?? STATE_LABEL.exited;
            const canStop = s.state === "running" || s.state === "starting";
            const lastOut = s.stdoutTail?.filter(Boolean).slice(-1)[0] ?? "";
            return (
              <div
                key={s.serviceId}
                className="rounded border border-[var(--inset-bg)] px-3 py-2"
              >
                <div className="flex items-center gap-2">
                  <span
                    className={`shrink-0 rounded px-1.5 py-0.5 text-xs font-medium ${badge.cls}`}
                  >
                    {badge.text}
                  </span>
                  <span
                    className="min-w-0 flex-1 truncate text-xs font-medium text-[var(--t2)]"
                    title={`${s.command} ${s.args.join(" ")}`}
                  >
                    {s.name || s.command}
                    {s.name && (
                      <span className="ml-1 font-normal text-[var(--t5)]">
                        {s.command}
                      </span>
                    )}
                  </span>
                  {canStop && (
                    <button
                      className="shrink-0 inline-flex items-center gap-1 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                      onClick={() => stop(s.serviceId)}
                      disabled={busyId !== ""}
                      aria-label={`停止服务 ${s.name || s.command}`}
                    >
                      <Square size={11} aria-hidden />
                      {busyId === s.serviceId ? "停止中…" : "停止"}
                    </button>
                  )}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-[var(--t5)]">
                  <span>pid {s.pid}</span>
                  <span>{formatUptime(s.uptimeSecs)}</span>
                  {s.rssMb !== undefined && (
                    <span>内存 {s.rssMb.toFixed(0)} MB</span>
                  )}
                  {s.cpuPct !== undefined && (
                    <span>CPU {s.cpuPct.toFixed(0)}%</span>
                  )}
                  {s.exitCode !== null && s.exitCode !== undefined && (
                    <span>退出码 {s.exitCode}</span>
                  )}
                </div>
                {s.warnings.length > 0 && (
                  <div className="mt-1 flex flex-wrap gap-2 text-xs text-[var(--danger)]">
                    {s.warnings.map((w) => (
                      <span key={w}>⚠ {WARNING_LABEL[w] ?? w}</span>
                    ))}
                  </div>
                )}
                {lastOut && (
                  <p
                    className="mt-1 truncate font-mono text-[11px] text-[var(--t5)]"
                    title={lastOut}
                  >
                    {lastOut}
                  </p>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
