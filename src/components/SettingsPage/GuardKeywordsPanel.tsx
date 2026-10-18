// SettingsPage 子模块：幻觉守卫「工具调用触发器关键词」卡（机器人分区）。
//
// 这张卡编辑的是守卫的**触发器**——终稿里出现表内任一串，才进「它说调了某个
// 工具、凭证表里查不查得到」那道判决。触发器不命中时判决根本不看凭证，所以
// 删掉一个词的真实代价是：该话术的谎报从此静默放行，而不是「判得更松一点」。
//
// 词表是全量替换不是合并：设置里的词表与生效值是同一份（registry
// ::claims_execution_with）。内置默认表由 view 带下来只读展示，「恢复默认」
// 只把词填回输入框、不自动写盘——用户看一眼再决定要不要保存。

import { RotateCcw } from "lucide-react";

type Props = {
  /** 当前生效词表（textarea 原始值：一行一个） */
  value: string;
  /** 内置默认词表（只读参照 + 「恢复默认」的取值来源） */
  defaults: string[];
  onChange: (next: string) => void;
  onSave: () => void;
  busy: boolean;
};

/** 按行拆词表：去空白、丢空行、去重（保持录入顺序） */
function parse(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || seen.has(line)) continue;
    seen.add(line);
    out.push(line);
  }
  return out;
}

export function GuardKeywordsPanel({
  value,
  defaults,
  onChange,
  onSave,
  busy,
}: Props) {
  const words = parse(value);
  const isDefault =
    words.length === defaults.length && words.every((w) => defaults.includes(w));

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">
        工具调用触发器关键词
      </h2>
      <p className="mt-1 text-xs leading-relaxed text-[var(--t5)]">
        模型回答里出现下面任意一个词，回复才会进入幻觉核查——核对它声称调用的
        工具在这次提问里到底跑没跑。一行一个，删掉即失效：该词对应的谎报会静默
        放行，且不留日志。
      </p>

      <div className="mt-4">
        <div className="mb-1.5 flex items-center justify-between gap-2">
          <p className="text-sm font-medium text-[var(--t2)]">
            当前生效（{words.length} 个）
          </p>
          <button
            type="button"
            onClick={() => onChange(defaults.join("\n"))}
            disabled={busy}
            className="inline-flex items-center gap-1 text-xs text-[var(--t4)] hover:text-[var(--t2)] disabled:opacity-50"
          >
            <RotateCcw size={12} />
            恢复默认
          </button>
        </div>
        <textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          rows={8}
          spellCheck={false}
          placeholder={"执行完毕\n跑完了\n已执行"}
          className="nm-inset w-full rounded-[var(--r-md)] px-3 py-2 font-mono text-xs leading-relaxed text-[var(--t1)] outline-none"
        />
        <p className="mt-1.5 text-xs text-[var(--t5)]">
          {words.length === 0
            ? "⚠️ 列表为空 = 关闭这道触发器，只剩「变更未调工具」与「路径不存在」两档核查。"
            : isDefault
              ? "与内置默认一致。"
              : "已偏离内置默认。"}
        </p>
      </div>

      <details className="mt-3">
        <summary className="cursor-pointer text-xs text-[var(--t4)] hover:text-[var(--t2)]">
          内置默认词表（{defaults.length} 个，只读参照）
        </summary>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {defaults.map((w) => (
            <code
              key={w}
              className="nm-inset rounded px-1.5 py-0.5 font-mono text-xs text-[var(--t3)]"
            >
              {w}
            </code>
          ))}
        </div>
        <p className="mt-2 text-xs leading-relaxed text-[var(--t5)]">
          内置表里有几条是刻意不加的——「返回了 / 结果是」会命中「这个函数返回了
          None」「1+1 的结果是 2」这类正常讲解，「执行完成」会命中「任务执行完成后
          会通知你」。删词之前值得先看一眼这里。
        </p>
      </details>

      <div className="mt-4 flex items-center gap-2">
        <button
          type="button"
          onClick={onSave}
          disabled={busy}
          className="rounded-[var(--r-md)] bg-[var(--brand)] px-4 py-1.5 text-sm font-medium text-white disabled:opacity-50"
        >
          {busy ? "保存中…" : "保存"}
        </button>
        <span className="text-xs text-[var(--t5)]">下次对话起生效</span>
      </div>
    </div>
  );
}
