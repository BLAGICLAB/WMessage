// SettingsPage 子模块：首页 KPI 指标卡（数据管理分区）。
// 8 项口径最多选 4 个展示，选中顺序即首页横排顺序（行内 ↑↓ 调序）；
// 口径说明就地展示，让「怎么算的」不用出设置页。localStorage 持久化 +
// 同文档事件即时同步首页（src/lib/homeKpi.ts），即点即生效。

import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import {
  HOME_KPIS,
  getHomeKpiSelection,
  setHomeKpiSelection,
  subscribeHomeKpiSelection,
  type KpiId,
} from "../../lib/homeKpi";

const MAX_SHOWN = 4;

export function HomeKpiPanel() {
  const [sel, setSel] = useState<KpiId[]>(() => getHomeKpiSelection());

  // 其他入口（未来挂件等）改了选择 → 事件同步回本页控件
  useEffect(() => subscribeHomeKpiSelection(() => setSel(getHomeKpiSelection())), []);

  const toggle = (id: KpiId) => {
    const next = sel.includes(id)
      ? sel.filter((x) => x !== id)
      : sel.length < MAX_SHOWN
        ? [...sel, id]
        : sel;
    setSel(setHomeKpiSelection(next));
  };

  const move = (id: KpiId, delta: -1 | 1) => {
    const i = sel.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= sel.length) return;
    const next = [...sel];
    [next[i], next[j]] = [next[j], next[i]];
    setSel(setHomeKpiSelection(next));
  };

  const iconBtn =
    "flex h-5 w-6 items-center justify-center rounded-[var(--r-sm)] text-[var(--t3)] disabled:opacity-30";

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">首页 KPI 指标</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        首页顶部横排展示的指标，最多 {MAX_SHOWN}{" "}
        个；涨跌角标对比的是上一次数值变动前的记录
      </p>
      <div className="mt-4 space-y-1">
        {HOME_KPIS.map((def) => {
          const idx = sel.indexOf(def.id);
          const chosen = idx !== -1;
          return (
            <div
              key={def.id}
              className="flex items-center justify-between gap-4 rounded-[var(--r-sm)] px-2 py-1.5"
            >
              <label className="flex min-w-0 cursor-pointer items-start gap-2.5">
                <input
                  type="checkbox"
                  className="mt-0.5 accent-[var(--t1)]"
                  checked={chosen}
                  disabled={!chosen && sel.length >= MAX_SHOWN}
                  onChange={() => toggle(def.id)}
                  aria-label={def.name}
                />
                <span className="min-w-0">
                  <span className="block text-sm font-medium text-[var(--t2)]">
                    {def.name}
                  </span>
                  <span className="mt-0.5 block text-xs text-[var(--t5)]">
                    {def.desc}
                  </span>
                </span>
              </label>
              {chosen && (
                <span className="flex shrink-0 items-center gap-1">
                  <span className="num text-xs text-[var(--t5)]">
                    第 {idx + 1}
                  </span>
                  <button
                    type="button"
                    className={`${iconBtn} nm-outset`}
                    disabled={idx === 0}
                    onClick={() => move(def.id, -1)}
                    aria-label={`上移${def.name}`}
                  >
                    <ChevronUp size={12} aria-hidden />
                  </button>
                  <button
                    type="button"
                    className={`${iconBtn} nm-outset`}
                    disabled={idx === sel.length - 1}
                    onClick={() => move(def.id, 1)}
                    aria-label={`下移${def.name}`}
                  >
                    <ChevronDown size={12} aria-hidden />
                  </button>
                </span>
              )}
            </div>
          );
        })}
      </div>
      <p className="mt-2 text-xs text-[var(--t5)]">
        已选 {sel.length}/{MAX_SHOWN}，顺序即首页展示顺序；改动即时生效并自动保存。
      </p>
    </div>
  );
}
