// SettingsPage 子模块：Word 模板参数编辑表单。
// 参数键与后端 WordTextStyle 同构（snake_case，直传 tauri）；底层单位是
// OOXML 原生的半磅/缇，界面讲公文话（号数/磅/字符），换算集中在文件顶部。

import { useState } from "react";

export interface WordTextStyle {
  font_east: string;
  font_ascii: string;
  size_half: number;
  bold: boolean;
  align: string;
  line_rule: string;
  line_line: number;
  indent_chars: number;
  before: number;
  after: number;
}

export interface WordTemplateParams {
  version: number;
  title: WordTextStyle;
  body: WordTextStyle;
  h1: WordTextStyle;
  h2: WordTextStyle;
  h3: WordTextStyle;
}

// 号数 → 磅（GB 公文字号表；半磅 = 磅×2）
const CN_SIZES: [string, number][] = [
  ["初号", 42],
  ["小初", 36],
  ["一号", 26],
  ["小一", 24],
  ["二号", 22],
  ["小二", 18],
  ["三号", 16],
  ["小三", 15],
  ["四号", 14],
  ["小四", 12],
  ["五号", 10.5],
  ["小五", 9],
];

const halfToPt = (half: number) => half / 2;
const ptToHalf = (pt: number) => Math.round(pt * 2);
const multipleToLine = (m: number) => Math.round(m * 240);
const lineToMultiple = (v: number) => Math.round((v / 240) * 100) / 100;
const ptToTwips = (pt: number) => Math.round(pt * 20);
const twipsToPt = (v: number) => Math.round((v / 20) * 10) / 10;

const ALIGN_OPTS: [string, string][] = [
  ["left", "居左"],
  ["center", "居中"],
  ["right", "居右"],
  ["both", "两端"],
];

const LINE_RULE_OPTS: [string, string][] = [
  ["auto", "倍数"],
  ["exact", "固定值(磅)"],
  ["atLeast", "最小值(磅)"],
];

const GROUPS: [keyof Omit<WordTemplateParams, "version">, string][] = [
  ["title", "主标题"],
  ["body", "正文"],
  ["h1", "一级标题"],
  ["h2", "二级标题"],
  ["h3", "三级标题"],
];

function num(v: number, step: number) {
  // 受控数字输入的值净化：NaN/负数按 0 处理，按步进取整避免浮点尾数
  if (!Number.isFinite(v) || v < 0) return 0;
  return Math.round(v / step) * step;
}

function StyleGroup({
  label,
  style,
  onChange,
}: {
  label: string;
  style: WordTextStyle;
  onChange: (next: WordTextStyle) => void;
}) {
  const set = (patch: Partial<WordTextStyle>) =>
    onChange({ ...style, ...patch });
  const pt = halfToPt(style.size_half);
  const inList = CN_SIZES.some(([, v]) => v === pt);
  const lineIsMultiple = style.line_rule === "auto";
  return (
    <div className="rounded-lg border border-[var(--line)] p-3">
      <div className="grid grid-cols-2 gap-x-3 gap-y-2 md:grid-cols-3">
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          中文字体
          <input
            className="nm-input px-2 py-1 text-xs min-w-0 flex-1"
            value={style.font_east}
            onChange={(e) => set({ font_east: e.target.value })}
          />
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          西文字体
          <input
            className="nm-input px-2 py-1 text-xs min-w-0 flex-1"
            value={style.font_ascii}
            onChange={(e) => set({ font_ascii: e.target.value })}
          />
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          字号
          <select
            className="nm-input px-2 py-1 text-xs"
            value={inList ? String(pt) : ""}
            onChange={(e) => {
              if (e.target.value)
                set({ size_half: ptToHalf(Number(e.target.value)) });
            }}
          >
            {!inList && <option value="">自定义（{pt} 磅）</option>}
            {CN_SIZES.map(([name, v]) => (
              <option key={v} value={v}>
                {name}（{v} 磅）
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          对齐
          <select
            className="nm-input px-2 py-1 text-xs"
            value={style.align}
            onChange={(e) => set({ align: e.target.value })}
          >
            {ALIGN_OPTS.map(([v, name]) => (
              <option key={v} value={v}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          行距
          <select
            className="nm-input px-2 py-1 text-xs"
            value={style.line_rule}
            onChange={(e) => set({ line_rule: e.target.value })}
          >
            {LINE_RULE_OPTS.map(([v, name]) => (
              <option key={v} value={v}>
                {name}
              </option>
            ))}
          </select>
          {lineIsMultiple ? (
            <input
              type="number"
              step={0.25}
              min={0}
              className="nm-input px-2 py-1 text-xs w-16"
              value={lineToMultiple(style.line_line)}
              onChange={(e) =>
                set({
                  line_line: multipleToLine(num(Number(e.target.value), 0.25)),
                })
              }
            />
          ) : (
            <input
              type="number"
              step={0.5}
              min={0}
              className="nm-input px-2 py-1 text-xs w-16"
              value={twipsToPt(style.line_line)}
              onChange={(e) =>
                set({ line_line: ptToTwips(num(Number(e.target.value), 0.5)) })
              }
            />
          )}
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          首行缩进
          <input
            type="number"
            step={0.5}
            min={0}
            className="nm-input px-2 py-1 text-xs w-16"
            value={style.indent_chars}
            onChange={(e) =>
              set({ indent_chars: num(Number(e.target.value), 0.5) })
            }
          />
          字符
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          段前
          <input
            type="number"
            step={0.5}
            min={0}
            className="nm-input px-2 py-1 text-xs w-16"
            value={twipsToPt(style.before)}
            onChange={(e) =>
              set({ before: ptToTwips(num(Number(e.target.value), 0.5)) })
            }
          />
          磅
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          段后
          <input
            type="number"
            step={0.5}
            min={0}
            className="nm-input px-2 py-1 text-xs w-16"
            value={twipsToPt(style.after)}
            onChange={(e) =>
              set({ after: ptToTwips(num(Number(e.target.value), 0.5)) })
            }
          />
          磅
        </label>
        <label className="flex items-center gap-1.5 text-xs text-[var(--t3)]">
          <input
            type="checkbox"
            checked={style.bold}
            onChange={(e) => set({ bold: e.target.checked })}
          />
          加粗
        </label>
      </div>
      <p className="sr-only">{label}</p>
    </div>
  );
}

export function WordTemplateStyleForm({
  params,
  onChange,
}: {
  params: WordTemplateParams;
  onChange: (next: WordTemplateParams) => void;
}) {
  const setGroup =
    (key: keyof Omit<WordTemplateParams, "version">) => (next: WordTextStyle) =>
      onChange({ ...params, [key]: next });
  const [openGroup, setOpenGroup] = useState<string>("title");
  return (
    <div className="space-y-2">
      {GROUPS.map(([key, label]) => (
        <details
          key={key}
          open={openGroup === key}
          onToggle={(e) => e.currentTarget.open && setOpenGroup(key)}
        >
          <summary className="cursor-pointer select-none text-xs font-medium text-[var(--t2)] py-1">
            {label}
            <span className="ml-2 text-[var(--t5)] font-normal">
              {params[key].font_east}·{halfToPt(params[key].size_half)}磅
              {params[key].bold ? "·加粗" : ""}
            </span>
          </summary>
          <StyleGroup
            label={label}
            style={params[key]}
            onChange={setGroup(key)}
          />
        </details>
      ))}
    </div>
  );
}
