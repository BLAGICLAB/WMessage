// SettingsPage 子模块：个人资料编辑行（头像 + 姓名）。
// 用户/机器人共用：kind 决定用 profile.user 还是 profile.bot。

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { setProfileName, setProfileAvatar, removeProfileAvatar } from "../../profile";
import { useProfile } from "../ActorAvatar";
import botLogo from "../../assets/main-logo.png";

export function ProfileRow({
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
