// SettingsPage 子模块：MCP（外部工具服务器）配置面板。
//
// 数据流：mcp_status 读「配置 × 连接槽」合并视图；增删改走 mcp_server_save/delete/toggle
//（后端落盘 + 自动重连，广播 mcp-status-changed 事件，本面板监听后刷新）。
// 权限模型（拍板 3A）：保存前显式确认弹窗完整展示将运行的命令行/参数/env。
// stdio 启动器白名单前后端双重把关（前端下拉只能选白名单内启动器）。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import {
  MCP_STDIO_LAUNCHERS,
  type McpServerConfig,
  type McpServerStatus,
  type McpToolBrief,
} from "./types";

/** 表单态：字符串形态（输入框原始值），保存时解析为 McpServerConfig */
type FormState = {
  id: string; // "" = 新建
  name: string;
  transport: string;
  command: string;
  /** 一行一个参数 */
  argsText: string;
  /** 一行一个 KEY=VALUE */
  envText: string;
  /** http：一行一个 Key: Value（鉴权头等） */
  headersText: string;
  url: string;
  /** 空串 = 默认 60s */
  timeoutSecs: string;
  enabled: boolean;
};

const EMPTY_FORM: FormState = {
  id: "",
  name: "",
  transport: "stdio",
  command: "npx",
  argsText: "",
  envText: "",
  headersText: "",
  url: "",
  timeoutSecs: "",
  enabled: true,
};

/** 一行一个参数 → string[]（trim、去空行） */
function parseArgsLines(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

/** KEY=VALUE / Key: Value 行 → Record 通用内核（首个分隔符切；缺分隔符行取整行为 key、空值） */
function parseKeyValueLines(text: string, sep: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const i = t.indexOf(sep);
    if (i < 0) {
      out[t] = "";
    } else {
      out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
    }
  }
  return out;
}

/** 一行一个 KEY=VALUE → Record */
const parseEnvLines = (text: string) => parseKeyValueLines(text, "=");

/** Key: Value 行 → Record（HTTP 头格式） */
const parseHeaderLines = (text: string) => parseKeyValueLines(text, ":");

/** 命令行展示（B0 评审 HIGH）：含空白的参数加引号，让展示 token 与实际 argv 对齐
 *（argv 是单 token 的 "hello world" 不能被展示成两个 shell 形态的词） */
function displayCommandLine(command: string | null | undefined, args: string[]): string {
  return [command ?? "", ...args]
    .map((a) => (/\s/.test(a) ? JSON.stringify(a) : a))
    .join(" ");
}

/** 头值打码（B0-4）：确认弹窗只露末 4 位，不展示完整鉴权机密 */
function maskHeaderValue(v: string): string {
  const t = v.trim();
  return t.length <= 8 ? "••••" : `••••••${t.slice(-4)}`;
}

/** 超时输入解析（B0-4）：空 = 默认；非法/越界报错（后端 u64 反序列化对
 * 负数/小数会裸抛 serde 错误，前端先钳为 5–600 整数） */
function parseTimeoutSecs(raw: string): { value: number | null; error?: string } {
  const t = raw.trim();
  if (!t) return { value: null };
  const n = Number(t);
  if (!Number.isInteger(n) || n < 5 || n > 600) {
    return { value: null, error: "超时须为 5–600 的整数秒" };
  }
  return { value: n };
}

/** 状态点：connected 绿 / down 红（tooltip 带错误）/ absent 灰 */
function StateDot({ status }: { status: McpServerStatus }) {
  const map: Record<string, { bg: string; tip: string }> = {
    connected: { bg: "bg-emerald-500", tip: "已连接" },
    down: { bg: "bg-red-500", tip: `不可用：${status.error ?? "未知错误"}` },
    absent: { bg: "bg-neutral-400", tip: status.enabled ? "未连接（启动中或等待）" : "已禁用" },
  };
  const m = map[status.state] ?? map.absent;
  return <span className={`size-2 shrink-0 rounded-full ${m.bg}`} title={m.tip} />;
}

/** 行内错误摘要：stderr 尾巴可能有多行（评审口径），列表行只显示前 80 字符 */
function errorSummary(err: string | null | undefined): string {
  const oneLine = (err ?? "").replace(/\s+/g, " ").trim();
  return oneLine.length > 80 ? `${oneLine.slice(0, 80)}…` : oneLine;
}

/** 保存确认弹窗内容（拍板 3A）：完整命令行 / URL + env，用户看得见才点得下去 */
function SaveConfirmDialog({
  server,
  onCancel,
  onConfirm,
  busy,
}: {
  server: McpServerConfig;
  onCancel: () => void;
  onConfirm: () => void;
  busy: boolean;
}) {
  const isStdio = server.transport === "stdio";
  const envEntries = Object.entries(server.env ?? {});
  const headerEntries = Object.entries(server.headers ?? {});
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
      <div className="nm-card w-full max-w-lg p-5">
        <h3 className="text-sm font-semibold text-[var(--t1)]">
          {server.id ? "保存修改" : "添加"} MCP 服务器「{server.name}」
        </h3>
        <p className="mt-1 text-xs text-[var(--t5)]">
          {isStdio
            ? "该服务器是一个本地程序，wmessage 将以你的身份运行它。请确认下面这条命令是你预期的："
            : "wmessage 将通过 HTTP 连接该端点并调用其工具："}
        </p>
        <div className="mt-3 rounded bg-black/5 p-3 font-mono text-xs break-all text-[var(--t2)]">
          {isStdio ? displayCommandLine(server.command, server.args ?? []) : server.url}
        </div>
        {envEntries.length > 0 && (
          <div className="mt-2 text-xs text-[var(--t4)]">
            环境变量（存系统钥匙串，不明文写配置文件）：
            {envEntries.map(([k, v]) => (
              <div key={k} className="mt-0.5 font-mono break-all">
                {k}={v}
              </div>
            ))}
          </div>
        )}
        {headerEntries.length > 0 && (
          <div className="mt-2 text-xs text-[var(--t4)]">
            自定义头（值已打码，鉴权头等机密将随请求发送）：
            {headerEntries.map(([k, v]) => (
              <div key={k} className="mt-0.5 font-mono break-all">
                {k}: {maskHeaderValue(v)}
              </div>
            ))}
          </div>
        )}
        <p className="mt-2 text-xs text-[var(--t5)]">
          该服务器的工具将进入机器人的工具清单，机器人可代你调用。env 与自定义头存系统钥匙串
          （不再明文写配置文件），删除或关闭开关即停用。
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <button className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]" onClick={onCancel}>
            取消
          </button>
          <button
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t1)] disabled:opacity-50"
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? "保存中…" : "确认，保存并连接"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** MCP 服务器管理面板：列表（状态点/启停/工具）+ 表单（确认弹窗后保存） */
export function McpPanel() {
  const [servers, setServers] = useState<McpServerStatus[]>([]);
  const [form, setForm] = useState<FormState | null>(null); // null = 表单收起
  const [pending, setPending] = useState<McpServerConfig | null>(null); // 确认弹窗
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  /** 展开工具清单的服务器 id + 已拉取的工具（toolsError = 拉取失败，行内展示） */
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [tools, setTools] = useState<McpToolBrief[]>([]);
  const [toolsError, setToolsError] = useState("");
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
    },
    []
  );

  const flashNotice = useCallback((msg: string) => {
    setNotice(msg);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(""), 3000);
  }, []);

  const refresh = useCallback(async () => {
    try {
      // ?? [] 防御：mock invoke / 后端异常时保持数组形态
      setServers((await invoke<McpServerStatus[]>("mcp_status")) ?? []);
    } catch (e) {
      handleCommandError(e, "mcp_status", { silent: true });
      setError(formatCommandError(e));
    }
  }, []);

  /** 800ms 延迟刷新（重连异步收敛）：单句柄防抖（B0 评审：数组累积无上界），卸载清理 */
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
    },
    []
  );
  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current) clearTimeout(refreshTimer.current);
    refreshTimer.current = setTimeout(refresh, 800);
  }, [refresh]);

  useEffect(() => {
    refresh();
    // 连接态是异步收敛的（启动重连 / 保存后重连），后端广播后刷新状态点
    const un = listen("mcp-status-changed", () => {
      refresh();
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  /** 编辑回填 in-flight 闸（B0 评审：连点两行会双 invoke 竞态 setForm——按钮的
   * disabled 要等首个 invoke 落地才生效，ref 闸同步无窗口） */
  const editLoadingRef = useRef(false);
  const startEdit = async (s: McpServerStatus) => {
    if (editLoadingRef.current || form !== null) return;
    editLoadingRef.current = true;
    setError("");
    // 表单回填需要原始配置字段——status 视图没有 args/env，
    // 从「编辑即整行覆盖」的角度只需回填用户可感知字段，args/env 留空会丢：
    // 因此编辑走 invoke bot_get_config 拿完整 mcpServers。
    try {
      const cfg = await invoke<{ mcpServers?: McpServerConfig[] }>("bot_get_config");
      const full = (cfg.mcpServers ?? []).find((x) => x.id === s.id);
      if (!full) {
        setError("找不到该服务器的完整配置");
        return;
      }
      setForm({
        id: full.id,
        name: full.name,
        transport: full.transport,
        command: full.command ?? "npx",
        argsText: (full.args ?? []).join("\n"),
        envText: Object.entries(full.env ?? {})
          .map(([k, v]) => `${k}=${v}`)
          .join("\n"),
        headersText: Object.entries(full.headers ?? {})
          .map(([k, v]) => `${k}: ${v}`)
          .join("\n"),
        url: full.url ?? "",
        timeoutSecs: full.timeoutSecs != null ? String(full.timeoutSecs) : "",
        enabled: full.enabled,
      });
    } catch (e) {
      handleCommandError(e, "bot_get_config", { silent: true });
      setError(formatCommandError(e));
    } finally {
      editLoadingRef.current = false;
    }
  };

  /** 表单 → McpServerConfig（前端轻校验；后端权威校验兜底） */
  const buildPayload = (): McpServerConfig | null => {
    if (!form) return null;
    const name = form.name.trim();
    if (!name) {
      setError("名称不能为空");
      return null;
    }
    // stdio 的 command 来自白名单下拉（MCP_STDIO_LAUNCHERS）恒非空，无需守卫
    //（B0 评审：原 !form.command 分支不可达，删除）
    if (form.transport !== "stdio" && !form.url.trim()) {
      setError("http 服务器必须填写端点 URL");
      return null;
    }
    const timeout = parseTimeoutSecs(form.timeoutSecs);
    if (timeout.error) {
      setError(timeout.error);
      return null;
    }
    return {
      id: form.id,
      name,
      transport: form.transport,
      command: form.transport === "stdio" ? form.command : null,
      args: form.transport === "stdio" ? parseArgsLines(form.argsText) : [],
      env: form.transport === "stdio" ? parseEnvLines(form.envText) : {},
      url: form.transport === "http" ? form.url.trim() : null,
      headers: form.transport === "http" ? parseHeaderLines(form.headersText) : {},
      timeoutSecs: timeout.value,
      enabled: form.enabled,
    };
  };

  const requestSave = () => {
    setError("");
    const payload = buildPayload();
    if (payload) setPending(payload); // 打开确认弹窗（拍板 3A）
  };

  const confirmSave = async () => {
    if (!pending || busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke<McpServerConfig[]>("mcp_server_save", { server: pending });
      setPending(null);
      setForm(null);
      flashNotice("已保存，正在连接…");
      // 后端 save 已触发重载；稍等一拍再刷状态（重载是异步的）
      scheduleRefresh();
      await refresh();
    } catch (e) {
      handleCommandError(e, "mcp_server_save", { silent: true });
      setError(formatCommandError(e));
      setPending(null); // 校验失败：关弹窗留表单，错误显示在表单里
    } finally {
      setBusy(false);
    }
  };

  const remove = async (s: McpServerStatus) => {
    if (busy) return;
    if (!window.confirm(`删除 MCP 服务器「${s.name}」？其工具将立即从机器人清单消失。`)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("mcp_server_delete", { id: s.id });
      flashNotice("已删除");
      await refresh();
    } catch (e) {
      handleCommandError(e, "mcp_server_delete", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = async (s: McpServerStatus, enabled: boolean) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("mcp_server_toggle", { id: s.id, enabled });
      await refresh();
      scheduleRefresh(); // 重连异步收敛
    } catch (e) {
      handleCommandError(e, "mcp_server_toggle", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  /** 展开/拉取工具：请求序号防串台（B0 评审：连点两台服务器，慢响应会把
   * A 的工具渲染进 B 的展开行）；失败在展开行内报错（此前只上顶栏，
   * 展开行一直显示「暂无工具」，分不清没工具还是拉取失败） */
  const toolsReqRef = useRef(0);
  const expandTools = async (s: McpServerStatus) => {
    if (expandedId === s.id) {
      setExpandedId(null);
      return;
    }
    const req = ++toolsReqRef.current;
    setExpandedId(s.id);
    setTools([]);
    setToolsError("");
    try {
      const t = await invoke<McpToolBrief[]>("mcp_server_tools", { id: s.id });
      if (toolsReqRef.current === req) setTools(t);
    } catch (e) {
      handleCommandError(e, "mcp_server_tools", { silent: true });
      if (toolsReqRef.current === req) setToolsError(formatCommandError(e));
    }
  };

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">MCP 服务器（外部工具）</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        连接外部 MCP 服务器，把它们的工具挂进机器人工具清单。启动器仅限
        {" "}{MCP_STDIO_LAUNCHERS.join(" / ")}；添加前会展示完整命令让你确认
      </p>
      <div className="mt-3 flex items-center gap-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
          onClick={() => {
            setError("");
            setForm({ ...EMPTY_FORM });
          }}
          disabled={form !== null}
        >
          ＋ 添加服务器
        </button>
        {notice && <span className="text-xs text-[var(--success)]">{notice}</span>}
        {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
      </div>

      {/* 服务器列表 */}
      {servers.length === 0 && form === null ? (
        <p className="mt-3 text-xs text-[var(--t5)]">
          暂无服务器：点「添加服务器」接入第一个 MCP 服务器
        </p>
      ) : (
        <div className="mt-3 flex flex-col gap-1.5">
          {servers.map((s) => (
            <div key={s.id} className="rounded border border-black/5 px-2 py-1.5">
              <div className="flex items-center gap-2">
                <StateDot status={s} />
                <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]">
                  <span className="font-medium text-[var(--t2)]">{s.name}</span>
                  <span className="text-[var(--t5)]">
                    {" "}
                    · {s.transport}
                    {s.enabled ? ` · ${s.toolCount} 个工具` : " · 已禁用"}
                    {s.state === "down" && s.error ? ` · ${errorSummary(s.error)}` : ""}
                  </span>
                </span>
                <button
                  className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={() => expandTools(s)}
                  disabled={!s.enabled}
                  title="查看已发现的工具"
                >
                  {expandedId === s.id ? "收起" : "工具"}
                </button>
                <button
                  className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={() => startEdit(s)}
                  disabled={busy || form !== null}
                  title="编辑"
                >
                  ✎
                </button>
                <button
                  className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                  onClick={() => remove(s)}
                  disabled={busy}
                  title="删除"
                >
                  🗑
                </button>
                {/* 启停开关：与设置页其他 Toggle 同款语义——点击即持久化 */}
                <label className="flex shrink-0 cursor-pointer items-center gap-1">
                  <input
                    type="checkbox"
                    className="accent-[var(--t1)]"
                    checked={s.enabled}
                    disabled={busy}
                    onChange={(e) => toggle(s, e.target.checked)}
                  />
                  <span className="text-xs text-[var(--t5)]">启用</span>
                </label>
              </div>
              {expandedId === s.id && (
                <div className="mt-1.5 border-t border-black/5 pt-1.5 text-xs text-[var(--t4)]">
                  {toolsError ? (
                    <span className="text-[var(--danger)]">工具获取失败：{toolsError}</span>
                  ) : tools.length === 0 ? (
                    <span className="text-[var(--t5)]">暂无工具（未连接或服务器未提供）</span>
                  ) : (
                    tools.map((t) => (
                      <div key={t.name} className="truncate">
                        <span className="font-mono text-[var(--t3)]">{t.name}</span>
                        {t.description && (
                          <span className="text-[var(--t5)]"> — {t.description}</span>
                        )}
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* 添加/编辑表单 */}
      {form && (
        <div className="mt-3 rounded border border-black/10 p-3">
          <div className="flex items-center justify-between">
            <p className="text-xs font-medium text-[var(--t2)]">
              {form.id ? "编辑服务器" : "新服务器"}
            </p>
            <button
              className="text-xs text-[var(--t5)] hover:text-[var(--t2)]"
              onClick={() => setForm(null)}
            >
              取消
            </button>
          </div>
          <div className="mt-2 grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2 text-xs">
            <label className="text-[var(--t4)]">名称</label>
            <input
              className="nm-input px-2 py-1 text-xs"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="如 filesystem"
            />
            <label className="text-[var(--t4)]">类型</label>
            <select
              className="nm-input px-2 py-1 text-xs"
              value={form.transport}
              onChange={(e) => setForm({ ...form, transport: e.target.value })}
            >
              <option value="stdio">stdio（本地进程）</option>
              <option value="http">http（远程端点）</option>
            </select>
            {form.transport === "stdio" ? (
              <>
                <label className="text-[var(--t4)]">启动命令</label>
                <select
                  className="nm-input px-2 py-1 text-xs"
                  value={form.command}
                  onChange={(e) => setForm({ ...form, command: e.target.value })}
                >
                  {MCP_STDIO_LAUNCHERS.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
                <label className="text-[var(--t4)]">参数</label>
                <textarea
                  className="nm-input px-2 py-1 font-mono text-xs"
                  rows={3}
                  value={form.argsText}
                  onChange={(e) => setForm({ ...form, argsText: e.target.value })}
                  placeholder={"一行一个参数，如：\n-y\n@modelcontextprotocol/server-filesystem\n/Users/me/Documents"}
                />
                <label className="text-[var(--t4)]">环境变量</label>
                <textarea
                  className="nm-input px-2 py-1 font-mono text-xs"
                  rows={2}
                  value={form.envText}
                  onChange={(e) => setForm({ ...form, envText: e.target.value })}
                  placeholder={"一行一个 KEY=VALUE，可选"}
                />
              </>
            ) : (
              <>
                <label className="text-[var(--t4)]">端点 URL</label>
                <input
                  className="nm-input px-2 py-1 font-mono text-xs"
                  value={form.url}
                  onChange={(e) => setForm({ ...form, url: e.target.value })}
                  placeholder="https://mcp.example.com/mcp（仅公网地址）"
                />
                <label className="text-[var(--t4)]">自定义头</label>
                <textarea
                  className="nm-input px-2 py-1 font-mono text-xs"
                  rows={2}
                  value={form.headersText}
                  onChange={(e) => setForm({ ...form, headersText: e.target.value })}
                  placeholder={"一行一个 Key: Value，鉴权头如：\nAuthorization: Bearer <token>"}
                />
              </>
            )}
            <label className="text-[var(--t4)]">超时（秒）</label>
            <input
              className="nm-input px-2 py-1 text-xs"
              value={form.timeoutSecs}
              onChange={(e) => setForm({ ...form, timeoutSecs: e.target.value })}
              placeholder="默认 60，可填 5–600"
            />
          </div>
          <div className="mt-3 flex justify-end">
            <button
              className="nm-btn px-3 py-1.5 text-xs text-[var(--t1)]"
              onClick={requestSave}
              disabled={busy}
            >
              {form.id ? "保存修改" : "下一步：确认并保存"}
            </button>
          </div>
        </div>
      )}

      {/* 保存确认弹窗（拍板 3A：完整命令展示后显式确认） */}
      {pending && (
        <SaveConfirmDialog
          server={pending}
          busy={busy}
          onCancel={() => setPending(null)}
          onConfirm={confirmSave}
        />
      )}
    </div>
  );
}
