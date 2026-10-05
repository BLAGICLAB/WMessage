// 任务图谱设置面板（G7-SETTINGS）：七项纯前端偏好（graphPrefs/localStorage），
// 打开图谱页时现读生效。开关/三档切换即时生效，无需保存按钮（同工作流可见性开关先例）。
import { useState } from "react";
import {
  getGraphAutoLayout,
  getGraphEdgeWidth,
  getGraphLabelDensity,
  getGraphLooseness,
  getGraphOnlyMine,
  getGraphRememberFilters,
  getGraphSizeMode,
  setGraphAutoLayout,
  setGraphEdgeWidth,
  setGraphLabelDensity,
  setGraphLooseness,
  setGraphOnlyMine,
  setGraphRememberFilters,
  setGraphSizeMode,
  type GraphEdgeWidth,
  type GraphLabelDensity,
  type GraphLooseness,
} from "../../lib/graphPrefs";
import type { GraphSizeMode } from "../GraphPage/graph-adapter";

function SwitchRow({
  label,
  desc,
  value,
  onChange,
}: {
  label: string;
  desc: string;
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
      <div className="min-w-0">
        <p className="text-sm font-medium text-[var(--t2)]">{label}</p>
        <p className="mt-1 text-xs text-[var(--t5)]">{desc}</p>
      </div>
      <button
        role="switch"
        aria-checked={value}
        aria-label={label}
        className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
          value ? "nm-inset" : "nm-outset"
        }`}
        onClick={() => onChange(!value)}
      >
        {value ? "已开启" : "已关闭"}
      </button>
    </div>
  );
}

function SegmentedRow<T extends string>({
  label,
  desc,
  value,
  options,
  onChange,
}: {
  label: string;
  desc: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
      <div className="min-w-0">
        <p className="text-sm font-medium text-[var(--t2)]">{label}</p>
        <p className="mt-1 text-xs text-[var(--t5)]">{desc}</p>
      </div>
      <div className="flex shrink-0 gap-1" role="radiogroup" aria-label={label}>
        {options.map((o) => (
          <button
            key={o.value}
            role="radio"
            aria-checked={value === o.value}
            className={`rounded-[var(--r-sm)] px-3 py-1 text-xs ${
              value === o.value
                ? "bg-[var(--inset-bg)] font-medium text-[var(--t1)]"
                : "text-[var(--t5)] hover:text-[var(--t2)]"
            }`}
            onClick={() => onChange(o.value)}
          >
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export default function GraphSettingsPanel() {
  const [onlyMine, setOnlyMine] = useState(getGraphOnlyMine);
  const [sizeMode, setSizeMode] = useState<GraphSizeMode>(getGraphSizeMode);
  const [labelDensity, setLabelDensity] = useState<GraphLabelDensity>(getGraphLabelDensity);
  const [edgeWidth, setEdgeWidth] = useState<GraphEdgeWidth>(getGraphEdgeWidth);
  const [autoLayout, setAutoLayout] = useState(getGraphAutoLayout);
  const [looseness, setLooseness] = useState<GraphLooseness>(getGraphLooseness);
  const [rememberFilters, setRememberFilters] = useState(getGraphRememberFilters);

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">任务图谱</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        图谱页（左侧导航「图谱」）的显示与布局偏好。全部即时生效，下次打开图谱即按新设置渲染。
      </p>
      <div className="mt-2 divide-y divide-[var(--edge)]">
        <SwitchRow
          label="只看我的任务"
          desc="打开图谱时成员过滤默认只留本人（多人汇总后快速切回个人视角）；图谱侧栏的成员筛选仍可临时加回他人"
          value={onlyMine}
          onChange={(v) => {
            setOnlyMine(v);
            setGraphOnlyMine(v);
          }}
        />
        <SegmentedRow<GraphSizeMode>
          label="节点大小"
          desc="连接度 = 连线越多点越大（经典关系图隐喻）；耗时 = 完成时间−创建时间天数越长点越大（进行中的任务按已进行天数）"
          value={sizeMode}
          options={[
            { value: "degree", label: "连接度" },
            { value: "duration", label: "耗时" },
          ]}
          onChange={(v) => {
            setSizeMode(v);
            setGraphSizeMode(v);
          }}
        />
        <SegmentedRow<GraphLabelDensity>
          label="标签密度"
          desc="节点任务名的显示多少：少 = 仅工作流和焦点邻域；多 = 全部显示。节点多时文字是第一个视觉洪水"
          value={labelDensity}
          options={[
            { value: "sparse", label: "少" },
            { value: "standard", label: "标准" },
            { value: "dense", label: "多" },
          ]}
          onChange={(v) => {
            setLabelDensity(v);
            setGraphLabelDensity(v);
          }}
        />
        <SegmentedRow<GraphEdgeWidth>
          label="连线粗细"
          desc="依赖/归属连线的线宽"
          value={edgeWidth}
          options={[
            { value: "thin", label: "细" },
            { value: "standard", label: "标准" },
            { value: "thick", label: "粗" },
          ]}
          onChange={(v) => {
            setEdgeWidth(v);
            setGraphEdgeWidth(v);
          }}
        />
        <SwitchRow
          label="打开时自动播放布局动画"
          desc="关闭后打开图谱即为静态分扇区布局（大图秒开、不烧 CPU）；图谱上的「重新布局」按钮仍可随时手动跑一轮"
          value={autoLayout}
          onChange={(v) => {
            setAutoLayout(v);
            setGraphAutoLayout(v);
          }}
        />
        <SegmentedRow<GraphLooseness>
          label="布局松散度"
          desc="节点分布的疏密：紧凑 = 团簇更聚拢；松散 = 节点间留更多空隙"
          value={looseness}
          options={[
            { value: "compact", label: "紧凑" },
            { value: "standard", label: "标准" },
            { value: "loose", label: "松散" },
          ]}
          onChange={(v) => {
            setLooseness(v);
            setGraphLooseness(v);
          }}
        />
        <SwitchRow
          label="记住上次的过滤器"
          desc="开启后图谱的整套过滤器（状态/成员/标签/年份/工作流/孤立节点）跨会话保留，下次打开原样恢复；侧栏「重置过滤器」可随时清空"
          value={rememberFilters}
          onChange={(v) => {
            setRememberFilters(v);
            setGraphRememberFilters(v);
          }}
        />
      </div>
    </div>
  );
}
