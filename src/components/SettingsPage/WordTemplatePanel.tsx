// SettingsPage 子模块：Word 模板管理（create_word 模板锚定的模板库）。
// 上传排好版的 .docx → 上传即提取文字排版参数（旁车 json，双层口径：docx 管
// 页面家具/参数管文字排版）；参数记录可编辑（名字+全部参数），生成时直接取参。
// 可设默认/删除。

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FileText, Pencil, RotateCw, Star, Trash2, Upload, X } from "lucide-react";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { EmptyState } from "../EmptyState";
import {
  WordTemplateStyleForm,
  type WordTemplateParams,
} from "./WordTemplateStyleForm";

interface WordTemplateInfo {
  name: string;
  mtimeMs: number;
  isDefault: boolean;
}

export function WordTemplatePanel() {
  const [items, setItems] = useState<WordTemplateInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editing, setEditing] = useState<{ name: string; params: WordTemplateParams } | null>(
    null,
  );
  const [editName, setEditName] = useState("");

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<WordTemplateInfo[]>("word_template_list");
      setItems(Array.isArray(list) ? list : []);
    } catch (e) {
      handleCommandError(e, "word_template_list", { silent: true });
      setError(formatCommandError(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const importTemplate = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Word 模板", extensions: ["docx"] }],
    });
    if (!picked || Array.isArray(picked)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("word_template_import", { path: picked });
      await refresh();
    } catch (e) {
      handleCommandError(e, "word_template_import");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const startEdit = async (name: string) => {
    setBusy(true);
    setError("");
    try {
      const params = await invoke<WordTemplateParams>("word_template_params_get", { name });
      setEditing({ name, params });
      setEditName(name);
    } catch (e) {
      handleCommandError(e, "word_template_params_get");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const saveEdit = async () => {
    if (!editing) return;
    setBusy(true);
    setError("");
    try {
      await invoke("word_template_params_update", {
        oldName: editing.name,
        newName: editName,
        params: editing.params,
      });
      setEditing(null);
      await refresh();
    } catch (e) {
      handleCommandError(e, "word_template_params_update");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const reextract = async () => {
    if (!editing) return;
    if (!window.confirm(`按「${editing.name}.docx」重新提取参数？将覆盖当前编辑内容`)) return;
    setBusy(true);
    setError("");
    try {
      const params = await invoke<WordTemplateParams>("word_template_reextract", {
        name: editing.name,
      });
      setEditing({ ...editing, params });
    } catch (e) {
      handleCommandError(e, "word_template_reextract");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const setDefault = async (name: string) => {
    setBusy(true);
    setError("");
    try {
      await invoke("word_template_set_default", { name });
      await refresh();
    } catch (e) {
      handleCommandError(e, "word_template_set_default");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (name: string) => {
    if (!window.confirm(`删除模板「${name}」？`)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("word_template_delete", { name });
      if (editing?.name === name) setEditing(null);
      await refresh();
    } catch (e) {
      handleCommandError(e, "word_template_delete");
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">Word 模板</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        上传排好版的 .docx 作为模板：docx 管页面设置/页眉页脚，文字排版（字体/字号/行距/缩进）
        上传时自动提取为参数，可编辑后生效。未设默认时使用内置版式
      </p>
      {error && <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>}
      {editing ? (
        <div className="mt-3">
          <div className="flex items-center gap-2">
            <input
              className="nm-input px-2 py-1 text-sm flex-1"
              value={editName}
              onChange={(e) => setEditName(e.target.value)}
              placeholder="模板名"
              aria-label="模板名"
            />
            <button
              className="nm-btn px-2 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
              onClick={() => void reextract()}
              disabled={busy}
              title="丢弃修改，按当前 docx 重新提取"
            >
              <RotateCw size={12} aria-hidden /> 重新提取
            </button>
            <button
              className="nm-btn px-2 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1"
              onClick={() => setEditing(null)}
              disabled={busy}
            >
              <X size={12} aria-hidden /> 取消
            </button>
            <button
              className="nm-btn px-3 py-1.5 text-xs inline-flex items-center gap-1 whitespace-nowrap"
              onClick={() => void saveEdit()}
              disabled={busy || editName.trim() === ""}
            >
              保存
            </button>
          </div>
          <div className="mt-2">
            <WordTemplateStyleForm
              params={editing.params}
              onChange={(params) => setEditing({ ...editing, params })}
            />
          </div>
        </div>
      ) : (
        <>
          <div className="mt-3 flex items-center gap-2">
            <button
              className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
              onClick={importTemplate}
              disabled={busy}
            >
              <Upload size={13} aria-hidden /> 上传模板
            </button>
          </div>
          {items.length === 0 ? (
            <div className="mt-3">
              <EmptyState
                icon={<FileText size={20} aria-hidden />}
                title="还没有模板"
                description="上传排好版的 .docx；机器人生成 Word 时版式随模板"
                action={{ label: "上传模板", onClick: () => void importTemplate() }}
              />
            </div>
          ) : (
            <ul className="mt-3 space-y-1.5">
              {items.map((t) => (
                <li
                  key={t.name}
                  className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm text-[var(--t2)] hover:bg-[var(--hover)]"
                >
                  <FileText size={14} aria-hidden className="shrink-0 text-[var(--t4)]" />
                  <span className="truncate">{t.name}</span>
                  {t.isDefault && (
                    <span className="shrink-0 rounded px-1.5 py-0.5 text-[10px] text-[var(--t5)]">
                      默认
                    </span>
                  )}
                  <span className="ml-auto shrink-0 flex items-center gap-1">
                    <button
                      className="nm-btn px-2 py-0.5 text-[10px]"
                      onClick={() => void startEdit(t.name)}
                      disabled={busy}
                    >
                      <Pencil size={11} aria-hidden className="mr-1 inline" />
                      编辑
                    </button>
                    {!t.isDefault && (
                      <button
                        className="nm-btn px-2 py-0.5 text-[10px]"
                        onClick={() => void setDefault(t.name)}
                        disabled={busy}
                      >
                        <Star size={11} aria-hidden className="mr-1 inline" />
                        设为默认
                      </button>
                    )}
                    <button
                      className="nm-btn px-2 py-0.5 text-[10px]"
                      onClick={() => void remove(t.name)}
                      disabled={busy}
                    >
                      <Trash2 size={11} aria-hidden className="mr-1 inline" />
                      删除
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
