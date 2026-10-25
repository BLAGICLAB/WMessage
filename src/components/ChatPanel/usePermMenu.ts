/** 聊天面板的「perm」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  授权模式下拉：当前授权模式与下拉开合。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "../../lib/errorHandler";
import type { PermMode } from "./types";

export function usePermMenu() {
  const [permMode, setPermMode] = useState<PermMode>("ask");
  const permModeRef = useRef<PermMode>("ask");
  const [permMenuOpen, setPermMenuOpen] = useState(false);
  const permBtnRef = useRef<HTMLButtonElement>(null);
  const permDropdownRef = useRef<HTMLDivElement>(null);
  const selectPermMode = useCallback((m: PermMode) => {
    setPermMenuOpen(false);
    const prev = permModeRef.current;
    if (prev === m) return;
    permModeRef.current = m;
    setPermMode(m);
    invoke("bot_set_perm_mode", { mode: m })
      .then(() => {})
      .catch((e) => {
        permModeRef.current = prev;
        setPermMode(prev);
        handleCommandError(e, "bot_set_perm_mode");
      });
  }, []);

  return {
    permBtnRef,
    permDropdownRef,
    permMenuOpen,
    permMode,
    permModeRef,
    selectPermMode,
    setPermMenuOpen,
    setPermMode,
  };
}
