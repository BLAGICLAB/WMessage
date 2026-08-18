import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
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
      setError(String(e));
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
      setError(String(e));
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
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const openDir = async () => {
    try {
      const dir = await invoke<string>("skills_open_dir");
      openPath(dir).catch(() => {});
    } catch (e) {
      setError(String(e));
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

type Props = {
  theme: ThemeSetting;
  onThemeChange: (t: ThemeSetting) => void;
  onExportTasks: () => Promise<void>;
  onImportTasks: () => Promise<void>;
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
      setError(String(e));
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
      setError(String(e));
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
      setError(String(e));
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
export function SettingsPage({ theme, onThemeChange, onExportTasks, onImportTasks }: Props) {
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
  const [botEnabled, setBotEnabled] = useState(false);
  const [botBusy, setBotBusy] = useState(false);
  const [botError, setBotError] = useState("");
  const [config, setConfig] = useState({
    baseUrl: "",
    model: "",
    hasApiKey: false,
    bypassLlmOnPreStepHit: true, // F-1 [P0] pre-step 路由外层是否跳过主 LLM；老配置默认 true
  });
  const [keyInput, setKeyInput] = useState("");
  const [configBusy, setConfigBusy] = useState(false);
  const [configSaved, setConfigSaved] = useState(false);
  const [pyEnabled, setPyEnabled] = useState(false);
  const [pyBusy, setPyBusy] = useState(false);
  const [pyEnv, setPyEnv] = useState<{ available: boolean; python: string; version: string; libs: string[] } | null>(null);
  const [pyEnvBusy, setPyEnvBusy] = useState(false);
  /** 环境检查失败的错误文案（可见反馈，不再静默 console.error） */
  const [pyEnvErr, setPyEnvErr] = useState("");
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState("");
  const [logBusy, setLogBusy] = useState(false);

  const refreshBot = async () => {
    try {
      const on = await invoke<boolean>("bot_get_enabled");
      setBotEnabled(on);
    } catch (e) {
      console.error("bot_get_enabled failed", e);
    }
  };

  const loadConfig = async () => {
    try {
      const c = await invoke<{
        baseUrl: string;
        model: string;
        hasApiKey: boolean;
        bypassLlmOnPreStepHit?: boolean;
      }>(
        "bot_get_config"
      );
      setConfig({
        baseUrl: c.baseUrl ?? "",
        model: c.model ?? "",
        hasApiKey: !!c.hasApiKey,
        // 老后端版本（没返 bypass 字段）默认 true，避免意外走 LEGACY 路径
        bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit ?? true,
      });
    } catch (e) {
      console.error("bot_get_config failed", e);
    }
  };

  const refreshPy = async () => {
    try {
      const on = await invoke<boolean>("py_get_enabled");
      setPyEnabled(on);
    } catch (e) {
      console.error("py_get_enabled failed", e);
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
      console.error("py_env_check failed", e);
      setPyEnvErr(String(e));
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
      setLogText(String(e));
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
      setBotError(String(e));
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
      setBotError(String(e));
      await refreshBot();
    } finally {
      setBotBusy(false);
    }
  };

  const saveConfig = async () => {
    if (configBusy) return;
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_set_config", {
        config: {
          baseUrl: config.baseUrl,
          model: config.model,
          bypassLlmOnPreStepHit: config.bypassLlmOnPreStepHit,
        },
        // 输入框非空才写凭据存储；留空保持原 key 不变
        apiKey: keyInput.trim() ? keyInput.trim() : null,
      });
      setKeyInput("");
      await loadConfig();
      setConfigSaved(true);
      setTimeout(() => setConfigSaved(false), 1500);
    } catch (e) {
      setBotError(String(e));
    } finally {
      setConfigBusy(false);
    }
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
      setBotError(String(e));
    } finally {
      setConfigBusy(false);
    }
  };

  const refresh = async () => {
    try {
      const s = await invoke<ApiStatus>("api_status");
      setStatus(s);
    } catch (e) {
      console.error("api_status failed", e);
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
      setError(String(e));
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

  const copyToken = async () => {
    try {
      await navigator.clipboard.writeText(status.token);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("clipboard failed", e);
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
      setError(String(e));
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
          任务卡上的归属头像：人完成 → 用户头像；交给机器人 → 机器人头像。悬停头像显示姓名。
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
            <p className="text-xs font-medium text-[var(--t4)]">大模型 API 配置（OpenAI 兼容）</p>
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">Base URL</p>
              <input
                value={config.baseUrl}
                onChange={(e) => setConfig((c) => ({ ...c, baseUrl: e.target.value }))}
                placeholder="https://api.deepseek.com/v1"
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
                placeholder="deepseek-chat"
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
            </div>
            {/* F-1 [P0 release blocker] bypass_llm_on_pre_step_hit Toggle */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">
                F‑1 开关：pre‑step 命中 Skill 时跳过外层主 LLM
              </p>
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
                  ? "ON · 新行为（推荐）"
                  : "OFF · LEGACY 旧链路"}
              </button>
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                ON（推荐）：auto 模式走 DSL 调度器，interactive 模式 LLM 驱动 Skill 步骤。<br />
                OFF（LEGACY 回退）：强制 pre_routed_skill = None，让 LLM 自由选 Skill（旧路径，紧急回退用）。
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                configBusy ? "nm-inset" : "nm-outset"
              }`}
              onClick={saveConfig}
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
