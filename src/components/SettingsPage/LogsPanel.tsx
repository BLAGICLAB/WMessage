// SettingsPage 子模块：日志（独立「日志」分类）。
//
// 收拢原先散在两处的日志入口：
//  - 基础设置里的「诊断」卡（error.log 尾部 + 一键复制全部日志）
//  - 机器人卡里的「机器人审计日志」（bot.log 尾部 + 全屏弹窗）
//
// 两者读的是同一种东西（磁盘上的日志尾部），合成一张卡：来源分段切换 +
// 一个定高日志窗 + 一个「复制全部日志」（error/bot/api/migration 含轮转历史
// 打包给开发者定位问题）。api.log 没有独立读命令，只在整包里出现，
// 所以不进分段切换——「复制全部日志」仍然带它。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, RefreshCw } from "lucide-react";
import { formatCommandError, handleCommandError } from "../../lib/errorHandler";
import { SettingCard, SET_LABEL_CLASS, SET_DESC_CLASS } from "./SettingRow";

const EMPTY_MARK = "（暂无日志）";

/** 可切读的日志来源：每项一条落盘文件名与读命令。api.log 没有独立读命令
 *  （只出现在整包里），所以不进分段切换——「复制全部日志」仍然带它。
 *  读取行数与诊断卡旧行为一致（200 行尾部）。 */
const LOG_SOURCES = [
  {
    key: "error",
    label: "错误日志",
    file: "error.log",
    describe: "程序记录的警告与错误，出现异常时先看这里",
  },
  {
    key: "bot",
    label: "机器人审计",
    file: "bot.log",
    describe: "机器人的指令、工具调用、参数与结果",
  },
  {
    key: "migration",
    label: "文件迁移",
    file: "migration.log",
    describe: "文件整理规则的每一次搬运与失败原因",
  },
] as const;

type SourceKey = (typeof LOG_SOURCES)[number]["key"];
const READ_LIMIT = 200;

export function LogsPanel() {
  const [source, setSource] = useState<SourceKey>("error");
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  /** 「已复制」复位计时器句柄（重复复制先清旧的，卸载时清理） */
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
    },
    [],
  );

  const active = LOG_SOURCES.find((s) => s.key === source) ?? LOG_SOURCES[0];

  /** 读一份日志。命令名按来源 switch 字面量给出，不走变量传参——
   *  tests-audit 的前端 invoke 静态提取靠字面量命令名发现桥接命令。 */
  const read = useCallback(async (src: SourceKey) => {
    const limit = { limit: READ_LIMIT };
    switch (src) {
      case "bot":
        return invoke<string>("bot_log_read", limit);
      case "migration":
        return invoke<string>("migration_log_read", limit);
      case "error":
      default:
        return invoke<string>("error_log_read", limit);
    }
  }, []);

  const reload = useCallback(
    (src: SourceKey) => {
      setBusy(true);
      setError("");
      read(src)
        .then((s) => {
          // 运行时可能不是 string（旧后端/未知命令返回 null）——按无内容处理
          setText(typeof s === "string" && s !== EMPTY_MARK ? s : "");
        })
        .catch((e) => {
          handleCommandError(e, `读取 ${src} 日志失败`, { silent: true });
          setError(formatCommandError(e));
          setText("");
        })
        .finally(() => setBusy(false));
    },
    [read],
  );

  useEffect(() => {
    // 挂载期拉一次：分类惰性挂载，首次进入才读盘
    // oxlint-disable-next-line react/set-state-in-effect
    reload(source);
  }, [reload, source]);

  const copyAll = async () => {
    setError("");
    try {
      const bundle = await invoke<string>("logs_bundle_text");
      await navigator.clipboard.writeText(bundle);
      setCopied(true);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      handleCommandError(e, "logs_bundle_text", { silent: true });
      setError(formatCommandError(e));
    }
  };

  return (
    <SettingCard
      title="日志"
      description="程序在本地记录的运行痕迹，用于定位问题；只读本机文件，不上传"
      actions={
        <div className="flex shrink-0 items-center gap-1.5">
          <button
            type="button"
            className="btn-sm-action nm-outset"
            onClick={() => reload(source)}
            disabled={busy}
          >
            <RefreshCw size={12} aria-hidden /> {busy ? "读取中…" : "刷新"}
          </button>
          <button
            type="button"
            className="btn-sm-action nm-outset"
            onClick={() => void copyAll()}
          >
            {copied ? (
              <>
                <Check size={12} aria-hidden /> 已复制
              </>
            ) : (
              <>
                <Copy size={12} aria-hidden /> 复制全部
              </>
            )}
          </button>
        </div>
      }
    >
      {/* 来源分段：三份日志共用一个读路径与同一个日志窗 */}
      <div
        role="radiogroup"
        aria-label="日志来源"
        className="flex gap-1"
      >
        {LOG_SOURCES.map((s) => (
          <button
            key={s.key}
            type="button"
            role="radio"
            aria-checked={source === s.key}
            onClick={() => setSource(s.key)}
            className={`flex-1 px-2 py-1.5 text-xs ${
              source === s.key ? "nm-inset" : "nm-outset"
            } text-[var(--t3)]`}
          >
            {s.label}
          </button>
        ))}
      </div>

      <div className="mt-3 space-y-2">
        <p className={SET_LABEL_CLASS}>{active.describe}</p>
        <p className={SET_DESC_CLASS}>
          数据目录下的 {active.file}，最新在前，最多 {READ_LIMIT} 行
        </p>
        <div className="nm-inset max-h-72 overflow-y-auto rounded-[var(--r-md)] p-3">
          {error ? (
            <p className="font-mono text-xs text-[var(--danger)]">{error}</p>
          ) : text === null ? (
            <p className="text-xs text-[var(--t5)]">读取中…</p>
          ) : text.length === 0 ? (
            <p className="text-xs text-[var(--t5)]">暂无记录</p>
          ) : (
            <pre className="whitespace-pre-wrap break-all font-mono text-xs leading-relaxed text-[var(--t3)]">
              {text}
            </pre>
          )}
        </div>
      </div>
    </SettingCard>
  );
}
