// SettingsPage 子模块：个人资料编辑行（头像 + 姓名）。
// 用户/机器人共用：kind 决定用 profile.user 还是 profile.bot。
// 头像为预设 ID 制：点头像弹选择器（16 选 1 即点即换），不再支持上传图片。

import { useEffect, useRef, useState } from "react";
import { Check } from "lucide-react";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import {
  setProfileName,
  setProfileAvatar,
  removeProfileAvatar,
} from "../../profile";
import { useProfile } from "../ActorAvatar";
import { AvatarPicker } from "./AvatarPicker";
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
  /** 「已保存 ✓」复位计时器句柄：重复保存先 clear 旧的，卸载时清理（防卸载后 setState） */
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (savedTimer.current) clearTimeout(savedTimer.current);
    },
    [],
  );

  // 资料加载/变更后回填姓名（编辑中不回填，避免覆盖输入）；
  // props→state 受控同步，改写为派生会改变输入竞态语义，保直写
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    if (entry && !dirty) setName(entry.name);
  }, [entry, dirty]);

  const pickAvatar = async (id: string) => {
    if (busy) return;
    setError("");
    try {
      await setProfileAvatar(kind, id);
    } catch (e) {
      handleCommandError(e, `profile_set_avatar:${kind}`, { silent: true });
      setError(formatCommandError(e));
    }
  };

  const clearAvatar = async () => {
    if (busy) return;
    setError("");
    try {
      await removeProfileAvatar(kind);
    } catch (e) {
      handleCommandError(e, `profile_remove_avatar:${kind}`, { silent: true });
      setError(formatCommandError(e));
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
      if (savedTimer.current) clearTimeout(savedTimer.current);
      savedTimer.current = setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      handleCommandError(e, `profile_set_name:${kind}`, { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const displayName = entry?.name || defaultName;

  return (
    <div className="flex items-center gap-3">
      <AvatarPicker
        kindLabel={label}
        avatarId={entry?.avatarId ?? null}
        disabled={busy}
        onPick={(id) => void pickAvatar(id)}
        onClear={() => void clearAvatar()}
        fallback={
          kind === "bot" ? (
            <img
              src={botLogo}
              alt={label}
              className="h-full w-full object-cover"
            />
          ) : (
            <span className="flex h-full w-full items-center justify-center text-sm text-[var(--t4)]">
              {displayName.charAt(0)}
            </span>
          )
        }
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <p className="w-10 shrink-0 text-sm font-medium text-[var(--t2)]">
            {label}
          </p>
          <input
            value={name}
            disabled={busy}
            maxLength={64}
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
            {saved ? (
              <>
                <Check size={11} aria-hidden /> 已保存
              </>
            ) : busy ? (
              "…"
            ) : (
              "保存"
            )}
          </button>
        </div>
        {error && (
          <p className="mt-1 pl-12 text-xs text-[var(--danger)]">{error}</p>
        )}
      </div>
    </div>
  );
}
