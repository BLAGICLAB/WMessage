// ChatPanel 子模块：斜杠命令 picker + 输入卡（U3a 拆分自 ChatPanel.tsx，JSX 逐字搬移）。
// 上多行输入区（自动增高）、附件 chips（图片悬停缩略图）、下工具栏
// （＋ 附件 / 🧠 模型下拉 / ⚡ 推理强度 / 发送-停止一体键）。
// 发送/停止的调用语义由父级闭包提供，本组件不做任何数据流决策。

import type { Dispatch, RefObject, SetStateAction } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { basename } from "../../format";
import { isImagePath } from "./UserBubbleContent";
import { EFFORT_LABELS, PERM_LABELS, PROVIDER_LABELS, SLASH_COMMANDS } from "./constants";
import type { ModelItem, PermMode, ReasoningLevel } from "./types";

/** 输入框占位文案：回复中 / 带附件 / 选任务 / 默认 四态（原嵌套三元提取） */
function placeholderOf(viewedBusy: boolean, hasFiles: boolean, selecting: boolean): string {
  if (viewedBusy) return "回复中…（点右侧 ■ 或输入 /stop 可停止）";
  if (hasFiles) return "输入指令，如：润色这个文件";
  if (selecting) return "输入操作指令，如：标记完成";
  return "和机器人说点什么";
}

/** 当前输入命中的斜杠命令（picker 渲染与键盘导航共用同一份匹配结果） */
function slashMatches(input: string) {
  return SLASH_COMMANDS.filter((c) => c.cmd.startsWith(input));
}

type InputAreaProps = {
  input: string;
  setInput: (v: string) => void;
  files: string[];
  setFiles: (v: string[] | ((prev: string[]) => string[])) => void;
  /** 输入卡 textarea 引用（父级 useAutoGrow 自动增高） */
  taRef: RefObject<HTMLTextAreaElement | null>;
  slashIdx: number;
  setSlashIdx: Dispatch<SetStateAction<number>>;
  slashDismissed: boolean;
  setSlashDismissed: Dispatch<SetStateAction<boolean>>;
  send: () => void;
  pickFiles: () => void;
  /** 停止当前回复（子 agent 会话由父级分派 cancel_subagent，见 WidgetApp 停止键语义） */
  onStop: () => void;
  viewedBusy: boolean;
  selecting: boolean;
  isSubagentSession: boolean;
  /** 🛡 授权模式只读展示（U3b）：bot-config 的 permMode，设置页维护 */
  permMode: PermMode;
  // 🧠/⚡ 的按钮与下拉 ref 顶层传（与滚动容器同模式）：ref 与普通值混嵌同一
  // prop 对象会触发 react/refs 的全对象标记
  modelBtnRef: RefObject<HTMLButtonElement | null>;
  modelDropdownRef: RefObject<HTMLDivElement | null>;
  effortBtnRef: RefObject<HTMLButtonElement | null>;
  effortDropdownRef: RefObject<HTMLDivElement | null>;
  model: {
    models: ModelItem[];
    provider: "openai" | "anthropic";
    activeId: string | null;
    label: string;
    menuOpen: boolean;
    setMenuOpen: Dispatch<SetStateAction<boolean>>;
    onSwitch: (id: string) => void;
  };
  effort: {
    base: ReasoningLevel;
    effective: ReasoningLevel;
    isOverridden: boolean;
    menuOpen: boolean;
    setMenuOpen: Dispatch<SetStateAction<boolean>>;
    setForSession: (level: ReasoningLevel | null) => void;
  };
};

export function InputArea({
  input,
  setInput,
  files,
  setFiles,
  taRef,
  slashIdx,
  setSlashIdx,
  slashDismissed,
  setSlashDismissed,
  send,
  pickFiles,
  onStop,
  viewedBusy,
  selecting,
  isSubagentSession,
  permMode,
  modelBtnRef,
  modelDropdownRef,
  effortBtnRef,
  effortDropdownRef,
  model,
  effort,
}: InputAreaProps) {
  return (
    <>
      {/* 斜杠命令 autocomplete：第一个字是 / 且无空格时浮出 picker。
          点选 / Tab / ↑↓ 选 / Esc 关；无匹配命令不渲染空壳容器 */}
      {!slashDismissed && input.startsWith("/") && !input.includes(" ") && (
        slashMatches(input).length > 0 && (
          <div className="mb-1.5 nm-card rounded-xl p-1 max-h-40 overflow-y-auto shrink-0">
            {slashMatches(input).map(
              (c, i) => (
                <button
                  key={c.cmd}
                  type="button"
                  className={`w-full flex flex-col items-start gap-0 px-2 py-1 rounded-lg text-left ${
                    i === slashIdx ? "nm-inset" : "hover:bg-[var(--hover-bg)]"
                  }`}
                  onMouseEnter={() => setSlashIdx(i)}
                  onClick={(e) => {
                    e.stopPropagation();
                    setInput(c.cmd + " ");
                    setSlashIdx(0);
                    setSlashDismissed(false);
                  }}
                >
                  <span className="text-xs text-[var(--t2)] font-medium">
                    {c.cmd}
                  </span>
                  <span className="text-[10px] text-[var(--t4)]">
                    {c.description}
                  </span>
                </button>
              )
            )}
          </div>
        )
      )}

      {/* 输入卡（U3b 换肤）：大圆角（20px）卡片一体式——上多行输入区（placeholder 左上、
          自动增高到 max-h-40 后内部滚动）、下工具栏（左 ➕ 附件，中 🛡 授权模式 +
          🧠 模型下拉 + ⚡ 推理强度，右圆形发送键）；斜杠 picker 仍浮在卡片上方 */}
      <div className="nm-card rounded-[20px] border border-[var(--edge)] p-2.5 shrink-0">
        {/* 已添加附件：随消息一起发送 */}
        {files.length > 0 && (
          <div className="mb-1.5 flex flex-wrap gap-1">
            {files.map((f) => {
              const isImg = isImagePath(f);
              return (
                <div key={f} className="relative group">
                  <span
                    className="nm-inset inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                    title={f}
                  >
                    <span className="truncate max-w-[280px]">
                      {isImg ? "🖼️" : "📎"} {basename(f)}
                    </span>
                    <button
                      type="button"
                      aria-label="移除附件"
                      className="text-[var(--t5)] hover:text-[var(--danger)]"
                      onClick={() => setFiles((prev) => prev.filter((x) => x !== f))}
                      title="移除"
                    >
                      ×
                    </button>
                  </span>
                  {/* 图片附件悬停缩略图：convertFileSrc 走 asset:// 协议。
                      tauri.conf.json 启用 assetProtocol.enable 后可读图；
                      未启用时 <img> 加载失败自然隐藏，chip 行为不变。 */}
                  {isImg && (
                    <img
                      src={convertFileSrc(f)}
                      alt={basename(f)}
                      loading="lazy"
                      onError={(e) => {
                        // 资产协议未启用（403）时隐藏占位元素
                        (e.currentTarget as HTMLImageElement).style.display = "none";
                      }}
                      className="pointer-events-none absolute bottom-full left-0 mb-1 hidden group-hover:block max-w-[220px] max-h-[120px] rounded-lg border border-[var(--border)] bg-[var(--bg)] shadow-lg object-contain z-10"
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
        <textarea
          ref={taRef}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setSlashIdx(0);
            setSlashDismissed(false);
          }}
          onKeyDown={(e) => {
            // 斜杠 picker 开着时拦截上下/Tab/Esc
            const pickerOpen =
              !slashDismissed && input.startsWith("/") && !input.includes(" ");
            const matches = slashMatches(input);
            if (pickerOpen && e.key === "ArrowDown" && matches.length) {
              e.preventDefault();
              setSlashIdx((slashIdx + 1) % matches.length);
              return;
            }
            if (pickerOpen && e.key === "ArrowUp" && matches.length) {
              e.preventDefault();
              setSlashIdx((slashIdx - 1 + matches.length) % matches.length);
              return;
            }
            if (pickerOpen && (e.key === "Tab" || e.key === "Enter") && matches.length) {
              // Tab 总是补全；Enter 在 picker 开着时也补全（避免输入半截命令误发送）
              e.preventDefault();
              setInput(matches[slashIdx].cmd + " ");
              setSlashIdx(0);
              return;
            }
            if (pickerOpen && e.key === "Escape") {
              e.preventDefault();
              setSlashDismissed(true);
              setSlashIdx(0);
              return;
            }
            // 与 TodoCard 一致：中文输入法组合态下回车确认候选词不应触发发送；
            // 多行输入（UI-1）：Shift+Enter 换行，Enter 发送（textarea 需手动阻止默认换行）
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              send();
            }
          }}
          placeholder={placeholderOf(viewedBusy, files.length > 0, selecting)}
          rows={1}
          className="w-full min-h-[48px] max-h-40 overflow-y-auto resize-none bg-transparent outline-none text-xs text-[var(--t2)] leading-relaxed placeholder:text-[var(--t4)]"
        />
        <div className="mt-1 flex items-center gap-1">
          <button
            type="button"
            aria-label="添加文件或图片"
            className="shrink-0 h-7 px-1.5 flex items-center justify-center rounded-lg text-lg leading-none text-[var(--t4)] hover:text-[var(--t2)] hover:bg-[var(--hover-bg)] transition-colors"
            title="添加文件或图片，和消息一起发送（如：添加 Word 后输入「润色」；图片发给机器人识别：png / jpg / jpeg / webp / gif / bmp，最大 3MB/张、最多 4 张/消息）"
            onClick={pickFiles}
            disabled={viewedBusy}
          >
            ＋
          </button>
          <div className="flex-1 min-w-0" />
          {/* 🛡 授权模式（U3b 只读展示）：读 bot-config 的 perm_mode，设置页维护；
              与 🧠/⚡ 同排的静态 pill，不承载操作 */}
          <span
            className="shrink-0 inline-flex items-center gap-1 rounded-full border border-[var(--edge)] px-2 py-0.5 text-[10px] leading-4 text-[var(--t4)]"
            title={`授权模式 ${permMode}（设置页维护）：ask=白名单外弹授权窗，strict=白名单外硬拒，yolo=全放行（仍记审计）`}
          >
            🛡 {PERM_LABELS[permMode]}
          </span>
          {/* 🧠 模型下拉（UI-1 从顶栏移入输入卡底栏；按钮/列表/切换逻辑不变，
              相对按钮向上弹出，免 JS 测量定位） */}
          <div className="relative shrink-0">
            <button
              type="button"
              ref={modelBtnRef}
              className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-[var(--t4)] hover:text-[var(--t2)] hover:bg-[var(--hover-bg)] transition-colors"
              title="切换模型"
              onClick={() => model.setMenuOpen((v) => !v)}
            >
              <span className="truncate max-w-[150px]">🧠 {model.label}</span>
              <span className="shrink-0 text-[10px]">
                {model.menuOpen ? "▴" : "▾"}
              </span>
            </button>
            {model.menuOpen && (
              <div
                ref={modelDropdownRef}
                className="absolute bottom-full right-0 mb-2 w-52 nm-card p-1 rounded-xl z-50 max-h-40 overflow-y-auto"
              >
                {model.models.length === 0 ? (
                  <p className="px-2 py-1 text-xs text-[var(--t5)]">
                    模型列表为空，去设置页添加
                  </p>
                ) : (
                  // MP-02：双协议同列——各协议一组（组头小字），当前协议下的
                  // active 模型高亮；跨协议选中由后端连协议一起切
                  (["openai", "anthropic"] as const).map((prov) => {
                    const list = model.models.filter((m) => m.provider === prov);
                    if (!list.length) return null;
                    return (
                      <div key={prov}>
                        <p className="px-2 pt-1.5 pb-0.5 text-[10px] text-[var(--t5)]">
                          {PROVIDER_LABELS[prov]}
                        </p>
                        {list.map((m) => {
                          const isActive =
                            prov === model.provider && m.id === model.activeId;
                          return (
                            <button
                              key={m.id}
                              className={`w-full text-left px-2 py-1 rounded-lg text-xs truncate ${
                                isActive
                                  ? "nm-inset text-[var(--t1)] font-medium"
                                  : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                              }`}
                              onClick={() => model.onSwitch(m.id)}
                              title={`${PROVIDER_LABELS[prov]} · ${m.label}`}
                            >
                              {m.label}
                            </button>
                          );
                        })}
                      </div>
                    );
                  })
                )}
              </div>
            )}
          </div>
          {/* ⚡ 推理强度（RE-1）：会话级覆盖下拉——默认跟随后台（缀「·默认」），
              选档只存本会话内存、不回写设置；向上弹出，交互镜像模型下拉 */}
          <div className="relative shrink-0">
            <button
              type="button"
              ref={effortBtnRef}
              className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-[var(--t4)] hover:text-[var(--t2)] hover:bg-[var(--hover-bg)] transition-colors"
              title="推理强度：只影响当前会话的发送，不写入设置"
              onClick={() => effort.setMenuOpen((v) => !v)}
            >
              <span>
                ⚡ {EFFORT_LABELS[effort.effective]}
                {effort.isOverridden ? "" : "·默认"}
              </span>
              <span className="shrink-0 text-[10px]">
                {effort.menuOpen ? "▴" : "▾"}
              </span>
            </button>
            {effort.menuOpen && (
              <div
                ref={effortDropdownRef}
                className="absolute bottom-full right-0 mb-2 w-44 nm-card p-1 rounded-xl z-50"
              >
                <button
                  className={`w-full text-left px-2 py-1 rounded-lg text-xs truncate ${
                    !effort.isOverridden
                      ? "nm-inset text-[var(--t1)] font-medium"
                      : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                  }`}
                  onClick={() => effort.setForSession(null)}
                  title={`清掉本会话覆盖，跟随后台默认（当前：${EFFORT_LABELS[effort.base]}）`}
                >
                  后台默认（{EFFORT_LABELS[effort.base]}）
                </button>
                {(["off", "low", "medium", "high"] as const).map((lv) => (
                  <button
                    key={lv}
                    className={`w-full text-left px-2 py-1 rounded-lg text-xs truncate ${
                      effort.effective === lv && effort.isOverridden
                        ? "nm-inset text-[var(--t1)] font-medium"
                        : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                    }`}
                    onClick={() => effort.setForSession(lv)}
                  >
                    {EFFORT_LABELS[lv]}
                  </button>
                ))}
              </div>
            )}
          </div>
          {/* 发送/停止一体键：回复中变为红描边圆形停止键，
              点击即 bot_stop 中断本次运行；中断/回答结束自动变回发送键 */}
          {viewedBusy ? (
            <button
              type="button"
              aria-label={isSubagentSession ? "取消子任务" : "停止当前回复"}
              className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center border border-[var(--danger)] text-[10px] text-[var(--danger)] hover:bg-[var(--hover-bg)] transition-colors"
              title={
                isSubagentSession
                  ? "取消子任务（cancel_subagent）"
                  : "停止当前回复"
              }
              onClick={onStop}
            >
              ■
            </button>
          ) : (
            <button
              type="button"
              className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-[var(--t1)] text-[var(--bg)] hover:opacity-85 transition-opacity"
              title="发送（Enter 发送，Shift+Enter 换行）"
              aria-label="发送"
              onClick={send}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M8 12.5v-9M3.8 7.7 8 3.5l4.2 4.2" />
              </svg>
            </button>
          )}
        </div>
      </div>
    </>
  );
}
