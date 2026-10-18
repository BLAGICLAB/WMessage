// 任务图谱设置面板（G7-SETTINGS）：六项纯前端偏好（graphPrefs/localStorage），
// 打开图谱页时现读生效。开关/三档切换即时生效，无需保存按钮。
import { useState } from "react";
import {
  getGraphAutoLayout,
  getGraphEdgeWidth,
  getGraphLabelDensity,
  getGraphLooseness,
  getGraphOnlyMine,
  getGraphRememberFilters,
  setGraphAutoLayout,
  setGraphEdgeWidth,
  setGraphLabelDensity,
  setGraphLooseness,
  setGraphOnlyMine,
  setGraphRememberFilters,
  type GraphEdgeWidth,
  type GraphLabelDensity,
  type GraphLooseness,
} from "../../lib/graphPrefs";

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
  const [labelDensity, setLabelDensity] =
    useState<GraphLabelDensity>(getGraphLabelDensity);
  const [edgeWidth, setEdgeWidth] = useState<GraphEdgeWidth>(getGraphEdgeWidth);
  const [autoLayout, setAutoLayout] = useState(getGraphAutoLayout);
  const [looseness, setLooseness] = useState<GraphLooseness>(getGraphLooseness);
  const [rememberFilters, setRememberFilters] = useState(
    getGraphRememberFilters,
  );

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">任务图谱</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        图谱页（左侧导航「图谱」）的显示与布局偏好。全部即时生效，下次打开图谱即按新设置渲染。
      </p>
      <div className="mt-2 divide-y divide-[var(--edge)]">
        <SwitchRow
          label="只看我的任务"
          desc="打开图谱时成员过滤默认只留本人（多人汇总后快速切回个人视角）；图谱侧栏的成员筛选仍可临时加回他人。若同时开启了「记住上次的过滤器」且已有存档，以存档为准"
          value={onlyMine}
          onChange={(v) => {
            setOnlyMine(v);
            setGraphOnlyMine(v);
          }}
        />
        <SegmentedRow<GraphLabelDensity>
          label="标签密度"
          desc="节点任务名的显示多少：少 = 仅搜索命中与选中邻域；标准 = 仅高连接度节点（度数前 7%）；多 = 全部显示。成员核/标签核/状态核的铭牌常驻不受影响。节点多时文字是第一个视觉洪水"
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
          desc="星轨连线的名义屏幕宽度：细 1px / 标准 1.6px / 粗 2.8px，纵深处按透视收细，近景约等于这个值。这一档只改粗细不改亮度——亮暗是星体的事，连线只管光路"
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
          desc="关闭后打开图谱直接是收敛好的静态三维构图（大图秒开、不烧 CPU）；图谱上的「重新布局」按钮仍可随时手动跑一轮"
          value={autoLayout}
          onChange={(v) => {
            setAutoLayout(v);
            setGraphAutoLayout(v);
          }}
        />
        <SegmentedRow<GraphLooseness>
          label="布局松散度"
          desc="这一档调的是星点的大小与亮暗，不改节点之间的排布——构图半径一变相机就同步后退，松紧差正好被抵消掉（实测相邻两档构图跨度只差 0.001%，星点直径却差 1.5 倍）。紧凑 = 星体更大更亮、看着更密实；宽松 = 星体更细更暗。任务多到星点挤成一团时，调紧凑更清楚"
          value={looseness}
          options={[
            { value: "tight", label: "紧凑" },
            { value: "standard", label: "标准" },
            { value: "loose", label: "宽松" },
          ]}
          onChange={(v) => {
            setLooseness(v);
            setGraphLooseness(v);
          }}
        />
        <SwitchRow
          label="记住上次的过滤器"
          desc="开启后图谱的整套过滤器（状态 / 成员 / 标签 / 完成年份 / 孤立节点）跨会话保留，下次打开原样恢复，优先级高于「只看我的任务」；侧栏「重置过滤器」可随时清空"
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
