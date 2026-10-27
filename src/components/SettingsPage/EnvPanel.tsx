// SettingsPage 子模块：本机运行环境卡。
//
// 三组各答一个问题：Python 包 / Node 全局包 / 运行时与启动器。
// 数据来自后端 `env_check`（src-tauri/src/env_probe.rs）。
//
// **为什么分组折叠**：全量清单 43 项（18 Python + 9 Node + 16 运行时）摊平
// 是一堵墙——真正要看的通常只是「哪个缺」。折叠后卡片保持一屏高，缺的那几项
// 单独给安装命令。

import { useState } from "react";
import type { EnvCheck } from "./types";

type Item = {
  name: string;
  ok: boolean;
  purpose: string;
  /** 产品硬依赖：缺了对应工具直接不可用（与「只是少个增强」要分开看） */
  required?: boolean;
};

type Group = {
  key: string;
  title: string;
  items: Item[];
  /** 缺失项的安装命令；null = 没有缺失项 */
  install: string | null;
};

export function EnvPanel({
  env,
  busy,
  error,
  onCheck,
}: {
  env: EnvCheck | null;
  busy: boolean;
  error: string;
  onCheck: () => Promise<void>;
}) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const groups = env && !env.failed ? buildGroups(env) : [];

  return (
    <div className="nm-card p-5">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold text-[var(--t1)]">
            本机运行环境
          </h2>
          <p className="mt-1 text-xs text-[var(--t5)]">
            {error
              ? `检查失败：${error}`
              : env
                ? env.failed
                  ? "探测异常，请点「检查环境」重试"
                  : summaryLine(env)
                : "点击右侧检查"}
          </p>
        </div>
        <button
          className={`btn-sm-action ${busy ? "nm-inset" : "nm-outset"}`}
          onClick={() => void onCheck()}
          disabled={busy}
        >
          {busy ? "检查中…" : "检查环境"}
        </button>
      </div>

      {groups.map((g) => (
        <EnvGroup
          key={g.key}
          group={g}
          open={!!open[g.key]}
          onToggle={() =>
            setOpen((prev) => ({ ...prev, [g.key]: !prev[g.key] }))
          }
        />
      ))}
    </div>
  );
}

function EnvGroup({
  group,
  open,
  onToggle,
}: {
  group: Group;
  open: boolean;
  onToggle: () => void;
}) {
  const installed = group.items.filter((i) => i.ok).length;
  const missingRequired = group.items.filter((i) => !i.ok && i.required);
  return (
    <div className="mt-3">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full items-center gap-2 text-xs text-[var(--t2)] hover:text-[var(--t1)]"
      >
        <span className="w-2 shrink-0 text-[var(--t5)]">
          {open ? "▾" : "▸"}
        </span>
        <span className="font-medium">{group.title}</span>
        <span className="text-[var(--t5)]">
          {installed}/{group.items.length} 已装
        </span>
        {missingRequired.length > 0 && (
          <span className="text-amber-600">
            {missingRequired.length} 项必需缺失
          </span>
        )}
      </button>
      {open && (
        <>
          <div className="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 sm:grid-cols-2">
            {group.items.map((it) => (
              <div
                key={it.name}
                className="flex items-baseline gap-1.5 text-[11px]"
              >
                <span
                  className={
                    it.ok
                      ? "text-emerald-600"
                      : it.required
                        ? "text-amber-600"
                        : "text-[var(--t5)]"
                  }
                >
                  {it.ok ? "✓" : "○"}
                </span>
                <span className="font-mono text-[var(--t3)]">{it.name}</span>
                <span className="truncate text-[var(--t5)]">{it.purpose}</span>
              </div>
            ))}
          </div>
          {group.install && (
            <p className="mt-2 break-all font-mono text-[11px] text-[var(--t5)]">
              安装缺失项：{group.install}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** 三组转成渲染用的分组（含缺失项安装命令） */
function buildGroups(env: EnvCheck): Group[] {
  const pyMissing = env.python.packages
    .filter((p) => !p.available)
    .map((p) => p.name);
  const nodeMissing = env.node.packages
    .filter((p) => !p.available)
    .map((p) => p.name);
  return [
    {
      key: "python",
      title: "Python 包",
      items: env.python.packages.map((p) => ({
        name: p.name,
        ok: p.available,
        purpose: p.purpose,
        required: p.required,
      })),
      install: pyMissing.length
        ? `python3 -m pip install ${pyMissing.join(" ")}`
        : null,
    },
    {
      key: "node",
      title: "Node 全局包",
      items: env.node.packages.map((p) => ({
        name: p.name,
        ok: p.available,
        purpose: p.purpose,
      })),
      install: nodeMissing.length
        ? `npm i -g ${nodeMissing.join(" ")}`
        : null,
    },
    {
      key: "launchers",
      title: "运行时与启动器",
      items: env.launchers.map((l) => ({
        name: l.name,
        ok: l.available,
        purpose: l.purpose,
      })),
      // 各命令的装法按平台而异（brew/apt/dotnet 官网…），不给统一命令
      install: null,
    },
  ];
}

/** 摘要行：解释器与运行时版本（探测失败的部分不报，免得看着像装好了） */
function summaryLine(env: EnvCheck): string {
  const parts: string[] = [];
  if (env.python.available) {
    parts.push(env.python.version || env.python.python);
  } else {
    parts.push("未检测到 Python");
  }
  if (env.node.available) {
    parts.push(
      env.node.version ? `Node ${env.node.version}` : "Node 已装",
    );
    if (env.node.npmVersion) parts.push(`npm ${env.node.npmVersion}`);
  }
  return parts.join(" · ");
}
