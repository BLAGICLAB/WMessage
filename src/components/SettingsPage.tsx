import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { handleCommandError, formatCommandError } from "../lib/errorHandler";
import { PROVIDER_PRESETS, matchProvider } from "../lib/providerPresets";
import type { ThemeSetting } from "../theme";
import { MigrationPanel } from "./MigrationPanel";
import { setProfileName, setProfileAvatar, removeProfileAvatar } from "../profile";
import { useProfile } from "./ActorAvatar";
import botLogo from "../assets/main-logo.png";

type ApiStatus = {
  enabled: boolean;
  port: number;
  token: string;
};

type SkillOutcomeKind = "done" | "await_user" | "failed_recoverable" | "terminated";
type SkillOutcome = {
  skillName: string;
  kind: SkillOutcomeKind;
  reason?: string;
  completedSummary?: string;
  rollbackAttempted?: boolean;
  lastAtMs: number;
};
type SkillInfo = { name: string; description: string; lastOutcome?: SkillOutcome | null };

/** Skill 状态徽章颜色 + 图标 (Phase 5 D 2026-08-18) */
function SkillOutcomeBadge({ outcome }: { outcome: SkillOutcome }) {
  const map: Record<SkillOutcomeKind, { color: string; label: string; icon: string }> = {
    done: { color: "text-emerald-600 bg-emerald-50", label: "完成", icon: "OK" },
    await_user: { color: "text-blue-600 bg-blue-50", label: "等待确认", icon: "PAUSE" },
    failed_recoverable: { color: "text-amber-600 bg-amber-50", label: "可恢复失败", icon: "WARN" },
    terminated: { color: "text-red-600 bg-red-50", label: "已终止", icon: "STOP" },
  };
  const m = map[outcome.kind];
  const tip = [m.label, outcome.reason, outcome.completedSummary].filter(Boolean).join(" | ");
  return (
    <span
      className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium ${m.color}`}
      title={tip}
    >
      [{m.icon}] {m.label}
    </span>
  );
}

/** 机器人技能管理：导入/删除/打开目录（技能 = 数据目录 skills/<name>/SKILL.md） */
function SkillsPanel() {
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const refresh = async () => {
    try {
      setSkills(await invoke<SkillInfo[]>("skills_list"));
    } catch (e) {
      // 自动加载失败：只 console 记 code，inline UI 仍展示 message
      handleCommandError(e, "skills_list", { silent: true });
      setError(formatCommandError(e));
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const importSkill = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const selected = await open({ multiple: false, directory: true });
      if (typeof selected !== "string") return;
      const name = await invoke<string>("skills_import", { path: selected });
      setNotice(`技能「${name}」已安装`);
      setTimeout(() => setNotice(""), 3000);
      await refresh();
    } catch (e) {
      handleCommandError(e, "skills_import", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const removeSkill = async (name: string) => {
    if (busy) return;
    if (!window.confirm(`删除技能「${name}」？`)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("skills_delete", { name });
      await refresh();
    } catch (e) {
      handleCommandError(e, "skills_delete", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const openDir = async () => {
    try {
      const dir = await invoke<string>("skills_open_dir");
      openPath(dir).catch(() => {});
    } catch (e) {
      handleCommandError(e, "skills_open_dir", { silent: true });
      setError(formatCommandError(e));
    }
  };

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">机器人技能</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        技能 = 一个文件夹（SKILL.md + 可选脚本）。机器人对话时自动看到技能清单，需要时读取完整文档执行
      </p>
      <div className="mt-3 flex items-center gap-2">
        <button className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]" onClick={importSkill} disabled={busy}>
          {busy ? "导入中…" : "⬆ 导入技能文件夹"}
        </button>
        <button className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]" onClick={openDir}>
          📂 打开技能目录
        </button>
        {notice && <span className="text-xs text-[var(--success)]">{notice}</span>}
        {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
      </div>
      {skills.length === 0 ? (
        <p className="mt-3 text-xs text-[var(--t5)]">
          暂无技能：点「导入技能文件夹」选择含 SKILL.md 的文件夹
        </p>
      ) : (
        <div className="mt-3 flex flex-col gap-1.5">
          {skills.map((s) => (
            <div key={s.name} className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]">
                <span className="font-medium text-[var(--t2)]">{s.name}</span>
                {s.description && (
                  <span className="text-[var(--t5)]"> — {s.description}</span>
                )}
              </span>
              {s.lastOutcome && <SkillOutcomeBadge outcome={s.lastOutcome} />}
              <button
                className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                onClick={() => removeSkill(s.name)}
                disabled={busy}
                title="删除技能"
              >
                🗑
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

const API_PROVIDER_OPTIONS = [
  { value: "openai", label: "OpenAI 兼容" },
  { value: "anthropic", label: "Anthropic 兼容" },
] as const;
type ApiProvider = (typeof API_PROVIDER_OPTIONS)[number]["value"];

/** API 协议下拉（2026-09-05）：原生 <select> 在 macOS 上弹系统级菜单——样式脱离
 *  新拟态主题、深浅色都不跟随，看起来像单独弹了个窗口。自绘下拉：触发钮 + 浮层
 *  全部走主题变量（nm-inset/nm-outset/var(--t*)），深浅色自动生效。
 *  交互：点击触发钮开合；点外部 / Esc 收起；点选项即选即收 */
function ApiProviderSelect({
  value,
  onChange,
}: {
  value: ApiProvider;
  onChange: (v: ApiProvider) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const current =
    API_PROVIDER_OPTIONS.find((o) => o.value === value) ?? API_PROVIDER_OPTIONS[0];
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none flex items-center justify-between gap-2"
        onClick={() => setOpen((v) => !v)}
      >
        <span className="truncate">{current.label}</span>
        <span
          className={`shrink-0 text-[var(--t5)] transition-transform ${open ? "rotate-180" : ""}`}
        >
          ▾
        </span>
      </button>
      {open && (
        <div className="nm-outset absolute z-50 mt-1 w-full p-1 space-y-0.5">
          {API_PROVIDER_OPTIONS.map((o) => (
            <button
              key={o.value}
              type="button"
              className={`w-full text-left px-3 py-1.5 text-xs rounded-lg ${
                o.value === value
                  ? "nm-inset text-[var(--t1)]"
                  : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
              }`}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

type Props = {
  theme: ThemeSetting;
  onThemeChange: (t: ThemeSetting) => void;
  onExportTasks: () => Promise<void>;
  onImportTasks: () => Promise<void>;
  /** 可选：未传时按钮置 disabled（App.tsx 已传；测试可选） */
  onExportWorkspace?: () => Promise<void>;
  onImportWorkspace?: () => Promise<void>;
};

/** 个人资料编辑行：头像预览 + 选图/移除 + 姓名输入 + 保存（用户/机器人共用） */
function ProfileRow({
  kind,
  label,
  defaultName,
}: {
  kind: "user" | "bot";
  label: string;
  defaultName: string;
}) {
  const profile = useProfile();
  const entry = profile ? (kind === "bot" ? profile.bot : profile.user) : null;
  const [name, setName] = useState("");
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  // 资料加载/变更后回填姓名（编辑中不回填，避免覆盖输入）
  useEffect(() => {
    if (entry && !dirty) setName(entry.name);
  }, [entry, dirty]);

  const pick = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
      });
      if (typeof selected === "string") {
        await setProfileAvatar(kind, selected);
      }
    } catch (e) {
      handleCommandError(e, `profile_set_avatar:${kind}`, { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await removeProfileAvatar(kind);
    } catch (e) {
      handleCommandError(e, `profile_remove_avatar:${kind}`, { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const saveName = async () => {
    if (busy) return;
    const n = name.trim();
    if (!n) {
      setError("姓名不能为空");
      return;
    }
    setBusy(true);
    setError("");
    try {
      await setProfileName(kind, n);
      setDirty(false);
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      handleCommandError(e, `profile_set_name:${kind}`, { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const src = entry?.avatarDataUrl;
  const displayName = entry?.name || defaultName;

  return (
    <div className="flex items-center gap-3">
      <span
        title={displayName}
        className="shrink-0 w-11 h-11 rounded-full overflow-hidden nm-inset flex items-center justify-center"
      >
        {src ? (
          <img src={src} alt={label} className="w-full h-full object-cover" />
        ) : kind === "bot" ? (
          <img src={botLogo} alt={label} className="w-full h-full object-cover" />
        ) : (
          <span className="text-sm text-[var(--t4)]">{displayName.charAt(0)}</span>
        )}
      </span>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="w-10 shrink-0 text-xs font-medium text-[var(--t4)]">{label}</p>
          <input
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setDirty(true);
            }}
            placeholder={displayName}
            className="nm-inset flex-1 min-w-0 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none"
          />
          <button
            className={`shrink-0 px-3 py-1.5 text-xs text-[var(--t3)] ${busy ? "nm-inset" : "nm-outset"}`}
            onClick={saveName}
            disabled={busy}
          >
            {saved ? "已保存 ✓" : busy ? "…" : "保存"}
          </button>
        </div>
        <div className="mt-1.5 flex items-center gap-2 pl-10">
          <button
            className="nm-btn px-2.5 py-1 text-[11px] text-[var(--t3)]"
            onClick={pick}
            disabled={busy}
          >
            🖼 选择图片
          </button>
          {src && (
            <button
              className="nm-btn px-2.5 py-1 text-[11px] text-[var(--danger)]"
              onClick={remove}
              disabled={busy}
            >
              移除头像
            </button>
          )}
          {error && <p className="text-[10px] text-[var(--danger)]">{error}</p>}
        </div>
      </div>
    </div>
  );
}

/** 设置页：个人资料 + 深浅色模式 + 任务数据管理 + 外部机器人 API 开关（默认关闭）+ token 展示与复制 */
export function SettingsPage({ theme, onThemeChange, onExportTasks, onImportTasks, onExportWorkspace, onImportWorkspace }: Props) {
  const [status, setStatus] = useState<ApiStatus>({
    enabled: false,
    port: 4763,
    token: "",
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [exportingWs, setExportingWs] = useState(false);
  const [importingWs, setImportingWs] = useState(false);
  const [botEnabled, setBotEnabled] = useState(false);
  const [botBusy, setBotBusy] = useState(false);
  const [botError, setBotError] = useState("");
  const [config, setConfig] = useState({
    baseUrl: "",
    model: "",
    hasApiKey: false,
    bypassLlmOnPreStepHit: true, // F-1 [P0] pre-step 路由外层是否跳过主 LLM；老配置默认 true
    // 本地文件工具白名单目录（textarea 一行一个；空 = 后端内置默认 桌面/下载/文档+绑定文件夹）
    allowedDirs: "",
    // Tavily 搜索 key 是否已存系统凭据存储（2026-09-05 起 key 本体不再回填，
    // 与主 API key 同模式：view 只给 has 标志，输入框独立 state 不回填）
    hasTavilyKey: false,
    // 「Tavily 搜索」开关（2026-08-20）：开 = web_search 走 Tavily；关 = Bing+百度双引擎
    tavilyEnabled: false,
    // Brave 搜索 key 是否已存系统凭据存储（2026-09-05，同 hasTavilyKey）
    hasBraveKey: false,
    // 「Brave 搜索」开关（2026-09-05）：开 = web_search 走 Brave；与 Tavily 互斥，双开报错
    braveEnabled: false,
    // run_python 默认超时秒数（空 = 60s 默认；模型 timeoutSecs 参数优先；硬钳 300s）
    pythonTimeoutSecs: "",
    // 授权模式（2026-08-26）：strict=白名单外硬拒 / ask=白名单外弹授权（默认）/ yolo=全放行
    permMode: "ask" as "strict" | "ask" | "yolo",
    // API 协议（2026-09-05）：openai=OpenAI 兼容（默认）/ anthropic=Anthropic 兼容
    apiProvider: "openai" as "openai" | "anthropic",
    // max_tokens（仅 Anthropic 模式用；空 = 8192 默认，范围 256-200000）
    maxTokens: "",
  });
  const [keyInput, setKeyInput] = useState("");
  // Tavily/Brave key 输入框（2026-09-05 起与主 keyInput 同模式：不回填已存 key，
  // 非空保存时覆盖写入系统凭据存储；空 = 不动已存 key）
  const [tavilyKeyInput, setTavilyKeyInput] = useState("");
  const [braveKeyInput, setBraveKeyInput] = useState("");
  const [configBusy, setConfigBusy] = useState(false);
  const [configSaved, setConfigSaved] = useState(false);
  const [pyEnabled, setPyEnabled] = useState(false);
  const [pyBusy, setPyBusy] = useState(false);
  const [pyEnv, setPyEnv] = useState<{ available: boolean; python: string; version: string; libs: string[] } | null>(null);
  const [pyEnvBusy, setPyEnvBusy] = useState(false);
  /** 环境检查失败的错误文案（可见反馈，不再只 console.error） */
  const [pyEnvErr, setPyEnvErr] = useState("");
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState("");
  const [logBusy, setLogBusy] = useState(false);
  // 预设行「自定义」按钮点击后聚焦 Base URL 输入框（对齐挂件模型菜单的自定义入口）
  const baseUrlInputRef = useRef<HTMLInputElement>(null);

  const refreshBot = async () => {
    try {
      const on = await invoke<boolean>("bot_get_enabled");
      setBotEnabled(on);
    } catch (e) {
      // 自动加载失败：只记 console，不打扰
      handleCommandError(e, "bot_get_enabled", { silent: true });
    }
  };

  const loadConfig = async () => {
    try {
      const c = await invoke<{
        baseUrl: string;
        model: string;
        hasApiKey: boolean;
        bypassLlmOnPreStepHit?: boolean;
        allowedDirs?: string[];
        // 2026-09-05 起 view 不再含 key 本体，只有 has 标志
        hasTavilyKey?: boolean;
        tavilyEnabled?: boolean | null;
        hasBraveKey?: boolean;
        braveEnabled?: boolean | null;
        pythonTimeoutSecs?: number | null;
        permMode?: string | null;
        apiProvider?: string | null;
        maxTokens?: number | null;
      }>(
        "bot_get_config"
      );
      setConfig({
        baseUrl: c.baseUrl ?? "",
        model: c.model ?? "",
        hasApiKey: !!c.hasApiKey,
        // 老后端版本（没返 bypass 字段）默认 true，避免意外走 LEGACY 路径
        bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit ?? true,
        allowedDirs: (c.allowedDirs ?? []).join("\n"),
        // 2026-09-05 起 view 只给 has 标志；key 本体不回填（与 hasApiKey/keyInput 同模式）。
        // 开关自动态：老配置没显式开关字段（null/undefined）时按 has 标志显示
        hasTavilyKey: c.hasTavilyKey ?? false,
        tavilyEnabled: c.tavilyEnabled ?? (c.hasTavilyKey ?? false),
        hasBraveKey: c.hasBraveKey ?? false,
        braveEnabled: c.braveEnabled ?? (c.hasBraveKey ?? false),
        pythonTimeoutSecs: c.pythonTimeoutSecs != null ? String(c.pythonTimeoutSecs) : "",
        // 老配置缺字段/非法值 → ask（与后端 PermMode::from_cfg 回退一致）
        permMode:
          c.permMode === "strict" || c.permMode === "yolo" ? c.permMode : "ask",
        // 老配置缺字段/非法值 → openai（与后端 ApiProvider::from_cfg 回退一致）
        apiProvider: c.apiProvider === "anthropic" ? "anthropic" : "openai",
        // 空 = 8192 默认（仅 Anthropic 模式用）
        maxTokens: c.maxTokens != null ? String(c.maxTokens) : "",
      });
    } catch (e) {
      handleCommandError(e, "bot_get_config", { silent: true });
    }
  };

  const refreshPy = async () => {
    try {
      const on = await invoke<boolean>("py_get_enabled");
      setPyEnabled(on);
    } catch (e) {
      handleCommandError(e, "py_get_enabled", { silent: true });
    }
  };

  const checkPyEnv = async () => {
    if (pyEnvBusy) return;
    setPyEnvBusy(true);
    setPyEnvErr("");
    try {
      const env = await invoke<{ available: boolean; python: string; version: string; libs: string[] }>("py_env_check");
      setPyEnv(env);
    } catch (e) {
      handleCommandError(e, "py_env_check", { silent: true });
      setPyEnvErr(formatCommandError(e));
    } finally {
      setPyEnvBusy(false);
    }
  };

  // 查看机器人审计日志（最新在前，默认 200 行）
  const openLog = async () => {
    if (logBusy) return;
    setLogBusy(true);
    try {
      setLogText(await invoke<string>("bot_log_read", { limit: 200 }));
    } catch (e) {
      handleCommandError(e, "bot_log_read", { silent: true });
      setLogText(formatCommandError(e));
    } finally {
      setLogBusy(false);
    }
    setLogOpen(true);
  };

  useEffect(() => {
    refreshBot();
    loadConfig();
    refreshPy();
  }, []);

  const togglePy = async () => {
    if (pyBusy) return;
    const enabling = !pyEnabled;
    if (enabling && !window.confirm("开启后机器人可执行 Python 代码（沙箱：独立临时目录 + 60 秒超时 + 审计留痕）。确认开启？")) return;
    setPyBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("py_set_enabled", { enabled: enabling });
      setPyEnabled(next);
      if (next) checkPyEnv();
    } catch (e) {
      handleCommandError(e, "py_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshPy();
    } finally {
      setPyBusy(false);
    }
  };

  const toggleBot = async () => {
    if (botBusy) return;
    setBotBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("bot_set_enabled", { enabled: !botEnabled });
      setBotEnabled(next);
      // 广播给挂件：展开状态下同步调整窗口高度（加/减聊天区）
      emit("bot-changed", next).catch(() => {});
    } catch (e) {
      handleCommandError(e, "bot_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshBot();
    } finally {
      setBotBusy(false);
    }
  };

  const saveConfig = async (override?: typeof config) => {
    if (configBusy) return;
    const c = override ?? config;
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_set_config", {
        config: {
          baseUrl: c.baseUrl,
          model: c.model,
          bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit,
          // textarea 一行一个路径；空行/空白剔除；全空 = 后端内置默认白名单
          allowedDirs: c.allowedDirs
            .split("\n")
            .map((s) => s.trim())
            .filter((s) => s.length > 0),
          // 空串视为未配置（后端 Option 语义）——2026-09-05 起 key 存系统凭据存储，
          // config 对象里的 key 字段固定传 null（后端强制置 None 双保险，不落明文）；
          // 新 key 走顶层 tavilyKey/braveKey 参数（见下方 invoke 调用）
          tavilyKey: null,
          // 开关显式落盘：开 = Tavily，关 = Bing+百度双引擎
          tavilyEnabled: c.tavilyEnabled,
          braveKey: null,
          // 开关显式落盘：开 = Brave，关 = 不走 Brave（与 Tavily 互斥，双开后端报错）
          braveEnabled: c.braveEnabled,
          // 空 = 60s 默认；非法输入按未配置处理（后端 resolve_timeout 硬钳 300s）
          pythonTimeoutSecs: (() => {
            const n = parseInt(c.pythonTimeoutSecs.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          // 授权模式（strict/ask/yolo）
          permMode: c.permMode,
          // API 协议（openai/anthropic，2026-09-05）
          apiProvider: c.apiProvider,
          // max_tokens：空 = 8192 默认；非法输入按未配置处理（后端钳 256..=200000）
          maxTokens: (() => {
            const n = parseInt(c.maxTokens.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
        },
        // 输入框非空才写凭据存储；留空保持原 key 不变
        apiKey: keyInput.trim() ? keyInput.trim() : null,
        // 2026-09-05：Tavily/Brave key 同主 key 模式——非空才覆盖写入系统凭据存储，
        // null = 不动已存 key（开关切换走 toggleTavily/toggleBrave → saveConfig，
        // 此时输入框为空 → keyring 不受任何影响）
        tavilyKey: tavilyKeyInput.trim() ? tavilyKeyInput.trim() : null,
        braveKey: braveKeyInput.trim() ? braveKeyInput.trim() : null,
      });
      setKeyInput("");
      setTavilyKeyInput("");
      setBraveKeyInput("");
      await loadConfig();
      // 广播给挂件聊天区：头部 🧠 模型标签同步刷新
      emit("bot-config-changed", null).catch(() => {});
      setConfigSaved(true);
      setTimeout(() => setConfigSaved(false), 1500);
    } catch (e) {
      handleCommandError(e, "bot_set_config", { silent: true });
      setBotError(formatCommandError(e));
    } finally {
      setConfigBusy(false);
    }
  };

  /** 「Tavily 搜索」开关：与「开启机器人聊天」同款——点击即持久化（复用整份配置保存） */
  const toggleTavily = async () => {
    const next = { ...config, tavilyEnabled: !config.tavilyEnabled };
    setConfig(next);
    await saveConfig(next);
  };

  /** 「Brave 搜索」开关（2026-09-05）：同 toggleTavily——点击即持久化（复用整份配置保存） */
  const toggleBrave = async () => {
    const next = { ...config, braveEnabled: !config.braveEnabled };
    setConfig(next);
    await saveConfig(next);
  };

  /** 授权模式切换（2026-08-26）：点击即持久化（同 toggleTavily 模式） */
  const setPermMode = async (mode: "strict" | "ask" | "yolo") => {
    if (configBusy || config.permMode === mode) return;
    const next = { ...config, permMode: mode };
    setConfig(next);
    await saveConfig(next);
  };

  const clearApiKey = async () => {
    if (configBusy) return;
    if (!window.confirm("清除已保存的 API Key？清除后机器人将无法调用大模型。")) return;
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_clear_api_key");
      await loadConfig();
    } catch (e) {
      handleCommandError(e, "bot_clear_api_key", { silent: true });
      setBotError(formatCommandError(e));
    } finally {
      setConfigBusy(false);
    }
  };

  const refresh = async () => {
    try {
      const s = await invoke<ApiStatus>("api_status");
      setStatus(s);
    } catch (e) {
      handleCommandError(e, "api_status", { silent: true });
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const toggle = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      if (status.enabled) {
        await invoke("api_stop");
      } else {
        await invoke("api_start");
      }
      await refresh();
    } catch (e) {
      handleCommandError(e, status.enabled ? "api_stop" : "api_start", { silent: true });
      setError(formatCommandError(e));
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const runExport = async () => {
    if (exporting) return;
    setExporting(true);
    try {
      await onExportTasks();
    } finally {
      setExporting(false);
    }
  };

  const runImport = async () => {
    if (importing) return;
    setImporting(true);
    try {
      await onImportTasks();
    } finally {
      setImporting(false);
    }
  };

  const runExportWs = async () => {
    if (exportingWs) return;
    if (!onExportWorkspace) return;
    setExportingWs(true);
    try {
      await onExportWorkspace();
    } finally {
      setExportingWs(false);
    }
  };

  const runImportWs = async () => {
    if (importingWs) return;
    if (!onImportWorkspace) return;
    setImportingWs(true);
    try {
      await onImportWorkspace();
    } finally {
      setImportingWs(false);
    }
  };

  const copyToken = async () => {
    try {
      await navigator.clipboard.writeText(status.token);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      handleCommandError(e, "clipboard", { silent: true });
    }
  };

  const rotateToken = async () => {
    if (busy) return;
    if (!window.confirm("重新生成 token？旧 token 会立即失效，已授权的客户端需要换新 token。")) return;
    setBusy(true);
    setError("");
    try {
      await invoke("api_rotate_token");
      await refresh();
    } catch (e) {
      handleCommandError(e, "api_rotate_token", { silent: true });
      setError(formatCommandError(e));
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      {/* 个人资料 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">个人资料</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          任务卡上的归属头像：设了定时 → 一直机器人头像；🤖 交给机器人执行中 → 机器人头像；执行完且无定时 → 用户头像。悬停头像显示姓名。
        </p>
        <div className="mt-4 space-y-4">
          <ProfileRow kind="user" label="用户" defaultName="我" />
          <ProfileRow kind="bot" label="机器人" defaultName="机器人" />
        </div>
      </div>

      {/* 深浅色模式 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">深浅色模式</h2>
        <div className="mt-4 flex gap-1">
          {(
            [
              ["light", "☀️ 浅色"],
              ["dark", "🌙 深色"],
              ["system", "🖥️ 跟随系统"],
            ] as [ThemeSetting, string][]
          ).map(([value, label]) => (
            <button
              key={value}
              className={`px-4 py-1.5 text-sm text-[var(--t3)] ${
                theme === value ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => onThemeChange(value)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {/* 任务数据管理 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">任务数据管理</h2>
        <div className="mt-4 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导出任务数据</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                全部任务卡（含归档、回收站）导出为 JSON 文件
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                exporting ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExport}
              disabled={exporting}
            >
              {exporting ? "导出中…" : "📤 导出"}
            </button>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导入任务数据</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                从 JSON 文件导入任务卡；按 id 合并，同 id 保留最后修改的记录
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                importing ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImport}
              disabled={importing}
            >
              {importing ? "导入中…" : "📥 导入"}
            </button>
          </div>
        </div>
      </div>

      {/* 工作区管理：与任务数据管理风格一致；workspace_items 数据独立于任务数据 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">工作区管理</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          工作区是按用途分组的链接集合（文件路径 / 文件夹 / 网页），与任务数据分开独立存储
        </p>
        <div className="mt-4 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导出工作区链接</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                全部工作区条目（含分组、折叠状态、链接列表）导出为 JSON 文件
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                exportingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExportWs}
              disabled={exportingWs}
            >
              {exportingWs ? "导出中…" : "📤 导出"}
            </button>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导入工作区链接</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                从 JSON 文件导入工作区条目；按 id 合并，同 id 保留最后修改的记录
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                importingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImportWs}
              disabled={importingWs}
            >
              {importingWs ? "导入中…" : "📥 导入"}
            </button>
          </div>
        </div>
      </div>

      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">机器人设置</h2>

        {/* 内置机器人聊天开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">开启机器人聊天</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              在挂件下方显示聊天窗口，用大模型管理任务
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              botEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleBot}
            disabled={botBusy}
          >
            {botBusy ? "…" : botEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {botError && (
          <p className="mt-2 text-xs text-[var(--danger)]">{botError}</p>
        )}

        {/* Python 编程开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">允许机器人执行 Python</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              文档处理/编程工具的本机执行开关；沙箱：独立临时目录 + 60 秒超时 + 审计留痕，默认关闭
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              pyEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={togglePy}
            disabled={pyBusy}
          >
            {pyBusy ? "…" : pyEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {/* Python 环境状态 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">本机 Python 环境</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              {pyEnvErr
                ? `检查失败：${pyEnvErr}`
                : pyEnv
                  ? pyEnv.available
                    ? `${pyEnv.version} · ${pyEnv.libs.join(" · ")}`
                    : "未检测到 Python（macOS 装 Command Line Tools；Windows 到 python.org 安装）"
                  : "点击右侧检查"}
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              pyEnvBusy ? "nm-inset" : "nm-outset"
            }`}
            onClick={checkPyEnv}
            disabled={pyEnvBusy}
          >
            {pyEnvBusy ? "检查中…" : "检查环境"}
          </button>
        </div>

        {/* Python 默认超时（2026-08-20：pandas 大计算 60s 偏紧；模型可用 timeoutSecs 参数临时调） */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">Python 默认超时（秒）</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              留空 = 60 秒；大计算可调大，上限 300 秒（超出自动钳制）
            </p>
          </div>
          <input
            value={config.pythonTimeoutSecs}
            onChange={(e) => setConfig((c) => ({ ...c, pythonTimeoutSecs: e.target.value }))}
            placeholder="60"
            inputMode="numeric"
            className="nm-inset shrink-0 w-24 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none text-right"
          />
        </div>

        {/* 机器人审计日志查看 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">机器人审计日志</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              记录机器人的用户指令、工具调用、参数与结果（数据目录 bot.log）
            </p>
          </div>
          <button
            className="shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] nm-outset"
            onClick={openLog}
            disabled={logBusy}
          >
            {logBusy ? "读取中…" : "查看日志"}
          </button>
        </div>

        {/* 大模型 API 配置（开关开启后显示） */}
        {botEnabled && (
          <div className="mt-4 border-t border-[var(--edge)] pt-4 space-y-3">
            <p className="text-xs font-medium text-[var(--t4)]">大模型 API 配置</p>
            {/* API 协议（2026-09-05 Anthropic 兼容模式）：自绘下拉（原生 select 弹系统菜单不跟随主题）；placeholder 随协议联动 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">API 协议</p>
              <ApiProviderSelect
                value={config.apiProvider}
                onChange={(v) => setConfig((c) => ({ ...c, apiProvider: v }))}
              />
            </div>
            {/* 提供商预设（Phase 3.3）：点击自动填 Base URL + 推荐模型，当前值命中的预设高亮 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">提供商预设</p>
              <div className="flex flex-wrap gap-2">
                {PROVIDER_PRESETS.map((p) => {
                  // 2026-09-05：预设收敛为供应商维度，高亮只看 baseUrl——
                  // 模型栏手改成同供应商其它型号时按钮保持高亮
                  const active = config.baseUrl.trim() === p.baseUrl;
                  return (
                    <button
                      key={p.label}
                      onClick={() =>
                        setConfig((c) => ({
                          ...c,
                          baseUrl: p.baseUrl,
                          model: p.model,
                          // 预设全是 OpenAI 兼容端点：点预设同时把协议拉回 openai，
                          // 防「Anthropic 协议 + OpenAI 端点」的错配组合
                          apiProvider: "openai",
                        }))
                      }
                      className={`px-3 py-1.5 text-xs rounded-xl ${active ? "nm-inset text-[var(--t1)]" : "nm-outset text-[var(--t3)]"}`}
                    >
                      {p.label}
                    </button>
                  );
                })}
                {/* 自定义（2026-09-05，对齐挂件模型菜单入口）：当前 baseUrl 不命中任何
                    供应商时高亮；点击清空 Base URL/模型并聚焦输入框，进入自定义填写态 */}
                <button
                  onClick={() => {
                    setConfig((c) => ({ ...c, baseUrl: "", model: "" }));
                    baseUrlInputRef.current?.focus();
                  }}
                  className={`px-3 py-1.5 text-xs rounded-xl ${
                    matchProvider(config.baseUrl) === undefined
                      ? "nm-inset text-[var(--t1)]"
                      : "nm-outset text-[var(--t3)]"
                  }`}
                >
                  ✏️ 自定义
                </button>
              </div>
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                点击自动填充 Base URL 和推荐模型，仍可手动修改；切换提供商记得换对应的 API Key。
              </p>
            </div>
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">Base URL</p>
              <input
                ref={baseUrlInputRef}
                value={config.baseUrl}
                onChange={(e) => setConfig((c) => ({ ...c, baseUrl: e.target.value }))}
                placeholder={
                  config.apiProvider === "anthropic"
                    ? "https://api.anthropic.com"
                    : "https://api.deepseek.com/v1"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
            </div>
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">
                API Key{config.hasApiKey && <span className="text-[var(--success)]"> · 已存入系统凭据存储 ✓</span>}
              </p>
              <input
                type="password"
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                placeholder={config.hasApiKey ? "已保存（输入新 Key 可覆盖）" : "sk-…"}
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.hasApiKey && (
                <button
                  className="text-[10px] text-[var(--danger)] hover:underline"
                  onClick={clearApiKey}
                >
                  清除已保存的 Key
                </button>
              )}
            </div>
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">模型</p>
              <input
                value={config.model}
                onChange={(e) => setConfig((c) => ({ ...c, model: e.target.value }))}
                placeholder={
                  config.apiProvider === "anthropic" ? "claude-sonnet-4-5" : "deepseek-v4-flash"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
            </div>
            {/* max_tokens（2026-09-05）：仅 Anthropic 模式显示（Anthropic 必填该字段；
                OpenAI 兼容模式不发送——多数兼容网关不认识） */}
            {config.apiProvider === "anthropic" && (
              <div className="space-y-1">
                <p className="text-[10px] text-[var(--t5)]">max_tokens</p>
                <input
                  value={config.maxTokens}
                  onChange={(e) => setConfig((c) => ({ ...c, maxTokens: e.target.value }))}
                  placeholder="8192"
                  inputMode="numeric"
                  className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                />
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  单次回复的最大 token 数（Anthropic 必填）。留空 = 8192；范围 256-200000，超出自动钳制。
                </p>
              </div>
            )}
            {/* 授权模式（2026-08-26，Kimi CLI 风格执行前授权） */}
            <div className="space-y-1">
              <p className="text-sm font-medium text-[var(--t2)]">授权模式</p>
              <div className="flex gap-2">
                {(
                  [
                    ["strict", "严格白名单"],
                    ["ask", "询问后放行"],
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
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                {config.permMode === "strict" &&
                  "白名单外的文件访问一律拒绝（2026-08-26 前的旧行为）。"}
                {config.permMode === "ask" &&
                  "白名单内的文件直接读；白名单外弹窗请你授权（允许一次 / 始终允许该目录 / 拒绝）。"}
                {config.permMode === "yolo" &&
                  "⚠️ 不弹任何授权：机器人可读本机任意文件，且 Python 编程免开关直接执行（以本机用户权限，可联网）。仅在你完全信任所用模型时开启。"}
              </p>
            </div>
            {/* 本地文件工具白名单（read_text_file/grep_files/list_files，2026-08-19 Phase 1；
                2026-08-26 起为追加语义：在内置默认之上追加放行） */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">文件工具白名单目录（一行一个绝对路径）</p>
              <textarea
                value={config.allowedDirs}
                onChange={(e) => setConfig((c) => ({ ...c, allowedDirs: e.target.value }))}
                placeholder={"在内置默认之上追加：~/Desktop、~/Downloads、~/Documents + 任务卡绑定文件夹始终放行"}
                rows={3}
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none resize-y"
              />
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                白名单内静默放行；白名单外按授权模式处理（见上）。授权弹窗点「始终允许该目录」会自动追加到这里。
              </p>
            </div>
            {/* Tavily 搜索（2026-08-19 Phase 2 key；2026-08-20 加开关分流） */}
            <div className="space-y-1">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-[var(--t2)]">Tavily 搜索</p>
                  <p className="mt-1 text-xs text-[var(--t5)]">
                    开启后 web_search 走 Tavily API（需填 key）；关闭走 Bing+百度双引擎
                  </p>
                </div>
                <button
                  className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                    config.tavilyEnabled ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={toggleTavily}
                  disabled={configBusy}
                >
                  {configBusy ? "…" : config.tavilyEnabled ? "已开启" : "已关闭"}
                </button>
              </div>
              <p className="text-[10px] text-[var(--t5)]">Tavily 搜索 API Key（可选）</p>
              <input
                type="password"
                value={tavilyKeyInput}
                onChange={(e) => setTavilyKeyInput(e.target.value)}
                placeholder={
                  config.hasTavilyKey
                    ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                    : "tvly-…（开关关闭时不使用）"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.tavilyEnabled && !config.hasTavilyKey && !tavilyKeyInput.trim() ? (
                <p className="text-[10px] text-[var(--danger)] leading-snug">
                  已开启但未填 key：web_search 会报错提示，请填写 key 后点「保存配置」，或关闭开关。
                </p>
              ) : (
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  key 存系统凭据存储（与主 API key 同），不再明文写进配置文件；填 key 后点「保存配置」生效；
                  要停用请关闭开关（已存的 key 不提供清除入口）。开关开启但缺 key / Tavily 请求失败时会明确报错，不会静默走百度。
                </p>
              )}
            </div>
            {/* Brave 搜索（2026-09-05，照搬 Tavily 模式；与 Tavily 互斥） */}
            <div className="space-y-1">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-[var(--t2)]">Brave 搜索</p>
                  <p className="mt-1 text-xs text-[var(--t5)]">
                    开启后 web_search 走 Brave Search API（需填 key）；关闭走 Bing+百度双引擎
                  </p>
                </div>
                <button
                  className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                    config.braveEnabled ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={toggleBrave}
                  disabled={configBusy}
                >
                  {configBusy ? "…" : config.braveEnabled ? "已开启" : "已关闭"}
                </button>
              </div>
              <p className="text-[10px] text-[var(--t5)]">Brave 搜索 API Key（可选）</p>
              <input
                type="password"
                value={braveKeyInput}
                onChange={(e) => setBraveKeyInput(e.target.value)}
                placeholder={
                  config.hasBraveKey
                    ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                    : "BSA…（https://brave.com/search/api/ 免费获取；开关关闭时不使用）"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.braveEnabled && !config.hasBraveKey && !braveKeyInput.trim() ? (
                <p className="text-[10px] text-[var(--danger)] leading-snug">
                  已开启但未填 key：web_search 会报错提示，请填写 key 后点「保存配置」，或关闭开关。
                </p>
              ) : (
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  key 存系统凭据存储（与主 API key 同），不再明文写进配置文件；填 key 后点「保存配置」生效；
                  要停用请关闭开关（已存的 key 不提供清除入口）。与 Tavily 互斥，两个开关同时开启会报错。开关开启但缺 key / Brave 请求失败时会明确报错，不会静默走百度。
                </p>
              )}
            </div>
            {/* Tavily/Brave 双开冲突提示（2026-09-05）：后端同样明确报错，这里提前可见 */}
            {config.tavilyEnabled && config.braveEnabled && (
              <p className="text-[10px] text-[var(--danger)] leading-snug">
                ⚠️ Tavily 与 Brave 只能开启一个，请关闭其中一个。
              </p>
            )}
            {/* F-1 bypass_llm_on_pre_step_hit 开关：技能路由新链路 / 旧链路回退闸 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">智能技能路由</p>
              <button
                className={`shrink-0 min-w-[160px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                  config.bypassLlmOnPreStepHit ? "nm-inset" : "nm-outset"
                }`}
                onClick={() =>
                  setConfig((c) => ({
                    ...c,
                    bypassLlmOnPreStepHit: !c.bypassLlmOnPreStepHit,
                  }))
                }
              >
                {config.bypassLlmOnPreStepHit
                  ? "已开启（推荐）"
                  : "已关闭（回退旧链路）"}
              </button>
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                开启（推荐）：命中技能时 auto 模式走 DSL 调度器、interactive 模式由 LLM 驱动技能步骤。<br />
                关闭：退回旧链路，由主 LLM 自由选择技能——仅当新路由行为异常时紧急回退用。
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                configBusy ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => saveConfig()}
              disabled={configBusy}
            >
              {configSaved ? "已保存 ✓" : configBusy ? "保存中…" : "保存配置"}
            </button>
          </div>
        )}

        {/* 外部机器人 API 开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              开启外部机器人 API 接口
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              {status.enabled
                ? `运行中 · http://127.0.0.1:${status.port}`
                : "默认关闭，开启后仅监听本机 127.0.0.1"}
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              status.enabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggle}
            disabled={busy}
          >
            {busy ? "…" : status.enabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {error && (
          <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>
        )}

        {/* token 展示 + 复制 */}
        {status.enabled && (
          <>
            <div className="mt-5">
              <p className="text-xs text-[var(--t4)]">访问令牌（Bearer Token）</p>
              <div className="mt-1.5 flex items-center gap-2">
                <input
                  readOnly
                  value={status.token}
                  className="nm-inset flex-1 min-w-0 rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                  onFocus={(e) => e.currentTarget.select()}
                />
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--t3)]"
                  onClick={copyToken}
                >
                  {copied ? "已复制 ✓" : "复制"}
                </button>
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--danger)]"
                  onClick={rotateToken}
                  disabled={busy}
                >
                  重新生成
                </button>
              </div>
            </div>

            {/* 接口说明 */}
            <div className="mt-5 space-y-1 text-xs text-[var(--t4)]">
              <p className="font-medium text-[var(--t3)]">
                接口（均需 Authorization: Bearer &lt;token&gt;）
              </p>
              <p>GET&nbsp;&nbsp;/api/health — 健康检查（免鉴权，仅服务状态）</p>
              <p>GET&nbsp;&nbsp;/api/tasks — 活跃任务（?status=todo|doing|done 筛选；?trash=1 回收站；?archived=1 归档；?all=1 全量）</p>
              <p>GET&nbsp;&nbsp;/api/tasks/:id — 单条任务</p>
              <p>POST&nbsp;/api/tasks — 新建任务（title 必填，note/status/filePath/fileIsDir/due/tags 可选）</p>
              <p>PUT&nbsp;&nbsp;/api/tasks/:id — 更新（title/note/status/filePath/fileIsDir/due/tags/archived/deleted）</p>
              <p>DELETE&nbsp;/api/tasks/:id — 软删进回收站（幂等）</p>
              <p>GET&nbsp;&nbsp;/api/events — SSE 实时推送（?since=事件id 断线重放）</p>
              <p className="mt-1 text-[var(--t5)]">开关状态自动记忆：退出时开启，下次启动自动恢复</p>
            </div>
          </>
        )}
      </div>

      <SkillsPanel />
      <MigrationPanel />

      {/* 机器人审计日志弹窗 */}
      {logOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
          <div className="nm-card w-full max-w-2xl h-[70vh] flex flex-col p-4">
            <div className="flex items-center justify-between mb-2 shrink-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                机器人审计日志（最新在前）
              </p>
              <div className="flex gap-2">
                <button
                  className="nm-btn px-3 py-1 text-xs text-[var(--t3)]"
                  onClick={openLog}
                >
                  刷新
                </button>
                <button
                  className="nm-btn px-3 py-1 text-xs text-[var(--t3)]"
                  onClick={() => setLogOpen(false)}
                >
                  关闭
                </button>
              </div>
            </div>
            <pre className="flex-1 min-h-0 overflow-auto text-[11px] leading-relaxed text-[var(--t4)] whitespace-pre-wrap break-all">
              {logText}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
