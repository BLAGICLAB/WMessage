import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openTarget } from "../lib/openTarget";
import { handleCommandError } from "../lib/errorHandler";
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import type { DraggableSyntheticListeners } from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { WorkspaceItem, WorkspaceLink } from "../types";
import { basename } from "../format";
import {
  loadWorkspaceFromDb,
  upsertWorkspaceItems,
  deleteWorkspaceRows,
  assignInsertOrder,
} from "../storage";
import { FoldToggle } from "./FoldToggle";

const stop = (e: React.PointerEvent) => e.stopPropagation();

/** 目标地址是否像网址（2026-09-05 修复：scheme 至少两字符——
 *  旧正则单字符即匹配，「C:」被当成 URL scheme，Windows 路径被误存成网址链接） */
const looksLikeUrl = (s: string) =>
  /^https?:\/\//i.test(s) || /^[a-z][a-z0-9+.-]+:/i.test(s);

/** 链接展示名：displayName 为空时回退（文件/文件夹用文件名，网址用地址本身） */
export function linkDisplayName(link: WorkspaceLink): string {
  const alias = link.displayName?.trim();
  if (alias) return alias;
  if (link.kind === "url") return link.targetUri;
  return basename(link.targetUri) || link.targetUri;
}

/** 工作区视图：类似任务卡的静态链接卡片（标题 + 右侧折叠开关 + 三列链接网格） */
export function WorkspacePage() {
  const [items, setItems] = useState<WorkspaceItem[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  /** 编辑中的链接（显示名称 + 目标地址双字段） */
  const [editingLink, setEditingLink] = useState<{
    itemId: string;
    linkId: string;
    displayName: string;
    targetUri: string;
    kind: WorkspaceLink["kind"];
  } | null>(null);
  /** 新增链接草稿（显示名称 + 目标地址） */
  const [draft, setDraft] = useState<{
    displayName: string;
    targetUri: string;
    kind: WorkspaceLink["kind"];
  }>({ displayName: "", targetUri: "", kind: "url" });
  const [draftFor, setDraftFor] = useState<string | null>(null);

  useEffect(() => {
    const reload = () => {
      loadWorkspaceFromDb().then(setItems);
    };
    reload();
    // 挂件改工作区（折叠/排序）后同步刷新（此前只加载一次，挂件改动不回显）
    const unlisten = listen("workspace-changed", reload);
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const persist = async (next: WorkspaceItem[]) => {
    setItems(next);
    await upsertWorkspaceItems(next);
    emit("workspace-changed").catch(() => {});
  };

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  /** 工作区上下排序：arrayMove 换位 → assignInsertOrder 分配 order → 行级落盘 + 广播挂件 */
  const handleSortEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = items.map((it) => it.id);
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    const byId = new Map(items.map((it) => [it.id, it]));
    const next = assignInsertOrder(
      arrayMove(ids, from, to).map((id) => byId.get(id)!),
      String(active.id)
    );
    setItems(next);
    const prevMap = new Map(items.map((it) => [it.id, it]));
    const changed = next.filter(
      (it) => JSON.stringify(it) !== JSON.stringify(prevMap.get(it.id))
    );
    const now = Date.now();
    changed.forEach((it) => (it.updatedAt = now));
    if (changed.length) {
      upsertWorkspaceItems(changed);
      emit("workspace-changed").catch(() => {});
    }
  };

  const addItem = () => {
    const now = Date.now();
    const max = items.reduce((m, t) => Math.max(m, t.order ?? 0), 0);
    const item: WorkspaceItem = {
      id: crypto.randomUUID(),
      title: "新工作区",
      collapsed: false,
      links: [],
      order: max + 1,
      updatedAt: now,
    };
    persist([...items, item]);
    setEditingId(item.id); // 新建后自动进入标题编辑态
  };

  const commitTitle = (id: string, title: string) => {
    const t = title.trim();
    setEditingId(null);
    if (!t) return;
    persist(
      items.map((it) =>
        it.id === id ? { ...it, title: t, updatedAt: Date.now() } : it
      )
    );
  };

  const removeItem = async (id: string) => {
    if (!window.confirm("删除这个工作区？其中的链接也会被移除。")) return;
    await deleteWorkspaceRows([id]);
    setItems((prev) => prev.filter((it) => it.id !== id));
    emit("workspace-changed").catch(() => {});
  };

  const toggleCollapsed = (id: string) => {
    persist(
      items.map((it) =>
        it.id === id
          ? { ...it, collapsed: !it.collapsed, updatedAt: Date.now() }
          : it
      )
    );
  };

  const updateLinks = (id: string, links: WorkspaceLink[]) => {
    persist(
      items.map((it) =>
        it.id === id ? { ...it, links, updatedAt: Date.now() } : it
      )
    );
  };

  /** 打开添加链接表单 */
  const openAddForm = (id: string) => {
    setDraftFor(id);
    setDraft({ displayName: "", targetUri: "", kind: "url" });
  };

  /** 对话框选文件/文件夹：targetUri 填路径，displayName 为空时自动预填文件名 */
  const pickLocal = async (directory: boolean) => {
    try {
      const selected = await open({ multiple: false, directory });
      if (typeof selected !== "string") return;
      const name = basename(selected);
      setDraft((prev) => ({
        displayName: prev.displayName.trim() ? prev.displayName : name,
        targetUri: selected,
        kind: directory ? "folder" : "file",
      }));
    } catch (e) {
      handleCommandError(e, "pick local");
    }
  };

  /** 提交新增链接：区分显示名称与真实目标 */
  const commitAddLink = (id: string) => {
    const it = items.find((x) => x.id === id);
    if (!it) return;
    const targetUri = draft.targetUri.trim();
    if (!targetUri) return;
    const displayName = draft.displayName.trim();
    const kind: WorkspaceLink["kind"] =
      draft.kind === "url" && !looksLikeUrl(targetUri)
        ? "file"
        : draft.kind;
    updateLinks(id, [
      ...it.links,
      { id: crypto.randomUUID(), displayName, targetUri, kind },
    ]);
    setDraft({ displayName: "", targetUri: "", kind: "url" });
    setDraftFor(null);
  };

  /** 提交编辑链接（别名随时可改） */
  const commitEditLink = () => {
    if (!editingLink) return;
    const it = items.find((x) => x.id === editingLink.itemId);
    if (!it) return;
    const targetUri = editingLink.targetUri.trim();
    if (!targetUri) {
      setEditingLink(null);
      return;
    }
    updateLinks(
      it.id,
      it.links.map((l) =>
        l.id === editingLink.linkId
          ? { ...l, displayName: editingLink.displayName.trim(), targetUri }
          : l
      )
    );
    setEditingLink(null);
  };

  // 2026-09-05：统一走 openTarget——按内容判定 URL/路径（不信任存储的 kind，
  // 历史数据可能 kind 错配），失败弹错不静默；文件仍走 Rust open_file_path
  // （前端 openPath 受 opener scope 限仅 $HOME/$APPDATA，D:\ 等路径会被拒）
  const openLink = (link: WorkspaceLink) => {
    openTarget(link.targetUri);
  };

  const removeLink = (id: string, linkId: string) => {
    const it = items.find((x) => x.id === id);
    if (!it) return;
    updateLinks(id, it.links.filter((l) => l.id !== linkId));
  };

  return (
    <div className="space-y-3">
      <button className="nm-btn w-full py-2 text-sm text-[var(--t3)]" onClick={addItem}>
        + 新建工作区
      </button>

      {items.length === 0 ? (
        <p className="text-xs text-[var(--t5)] text-center mt-10">
          暂无工作区：新建一个，把常用文件、文件夹、网址放进卡片里
        </p>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleSortEnd}
        >
          <SortableContext
            items={items.map((it) => it.id)}
            strategy={verticalListSortingStrategy}
          >
            {items.map((it) => (
              <SortableWorkspaceItem key={it.id} it={it}>
                {(listeners) => (
                  <div>
            {/* 标题行：☰ 拖拽手柄 + 标题 + 折叠开关 + 删除 */}
            <div className="flex items-center gap-2">
              <span
                {...listeners}
                title="拖拽排序"
                className="shrink-0 w-4 h-4 flex items-center justify-center text-[12px] leading-none text-[var(--t5)] rounded hover:bg-[var(--hover-bg)] opacity-0 group-hover:opacity-100 transition-opacity cursor-grab active:cursor-grabbing"
              >
                ☰
              </span>
              {editingId === it.id ? (
                <input
                  autoFocus
                  defaultValue={it.title}
                  className="nm-inset min-w-0 flex-1 rounded-xl px-3 py-1.5 text-sm font-medium text-[var(--t1)] outline-none"
                  onFocus={(e) => e.currentTarget.select()}
                  onBlur={(e) => commitTitle(it.id, e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter")
                      commitTitle(it.id, (e.target as HTMLInputElement).value);
                    if (e.key === "Escape") setEditingId(null);
                  }}
                />
              ) : (
                <button
                  className="min-w-0 flex-1 truncate text-left text-sm font-medium text-[var(--t1)]"
                  title="点击编辑标题"
                  onClick={() => setEditingId(it.id)}
                >
                  {it.title}
                </button>
              )}
              <FoldToggle collapsed={!!it.collapsed} onToggle={() => toggleCollapsed(it.id)} />
              <button
                className="shrink-0 w-5 h-5 flex items-center justify-center rounded-full text-xs text-[var(--t5)] hover:text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                title="删除工作区"
                onClick={() => removeItem(it.id)}
              >
                🗑️
              </button>
            </div>

            {/* 折叠展开内容：链接三列网格，从左到右 */}
            {!it.collapsed && (
              <div className="mt-3">
                {it.links.length === 0 && !editingLink && (
                  <p className="text-xs text-[var(--t5)]">还没有链接</p>
                )}

                {it.links.length > 0 && (
                  <div className="grid grid-cols-3 gap-2">
                    {it.links.map((link) =>
                      editingLink?.linkId === link.id ? (
                        /* 编辑态：占满三列，双字段表单 */
                        <div
                          key={link.id}
                          className="nm-inset col-span-3 flex flex-wrap items-center gap-2 rounded-xl px-3 py-2"
                        >
                          <span className="text-[10px] text-[var(--t5)]">显示名称</span>
                          <input
                            autoFocus
                            value={editingLink.displayName}
                            placeholder="别名（可自由修改）"
                            className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
                            onChange={(e) =>
                              setEditingLink({ ...editingLink, displayName: e.target.value })
                            }
                            onKeyDown={(e) => {
                              if (e.key === "Enter" && !e.nativeEvent.isComposing) commitEditLink();
                            }}
                            onPointerDown={stop}
                          />
                          <span className="text-[10px] text-[var(--t5)]">目标地址</span>
                          <input
                            value={editingLink.targetUri}
                            className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
                            onChange={(e) =>
                              setEditingLink({ ...editingLink, targetUri: e.target.value })
                            }
                            onKeyDown={(e) => {
                              if (e.key === "Enter" && !e.nativeEvent.isComposing) commitEditLink();
                            }}
                            onPointerDown={stop}
                          />
                          <button
                            className="nm-btn shrink-0 px-3 py-1 text-xs text-[var(--t3)]"
                            onClick={commitEditLink}
                            onPointerDown={stop}
                          >
                            保存
                          </button>
                          <button
                            className="shrink-0 px-2 py-1 text-xs text-[var(--t5)]"
                            onClick={() => setEditingLink(null)}
                            onPointerDown={stop}
                          >
                            取消
                          </button>
                        </div>
                      ) : (
                        <div
                          key={link.id}
                          className="nm-inset group/link relative rounded-xl px-2.5 py-2 cursor-pointer"
                          onClick={() => openLink(link)}
                        >
                          <div className="flex items-center gap-1.5">
                            <span className="shrink-0 text-xs">
                              {link.kind === "url" ? "🔗" : link.kind === "folder" ? "📁" : "📄"}
                            </span>
                            <span
                              className="min-w-0 flex-1 truncate text-xs font-medium text-[var(--t2)]"
                              title={linkDisplayName(link)}
                            >
                              {linkDisplayName(link)}
                            </span>
                          </div>
                          <p
                            className="mt-1 truncate text-[10px] text-[var(--t5)]"
                            title={link.targetUri}
                          >
                            {link.targetUri}
                          </p>
                          {/* 悬停操作：编辑别名 / 删除 */}
                          <div
                            className="absolute right-1 top-1 hidden group-hover/link:flex gap-0.5"
                            onPointerDown={stop}
                            onClick={(e) => e.stopPropagation()}
                          >
                            <button
                              className="w-5 h-5 flex items-center justify-center rounded-full text-[10px] text-[var(--t4)] hover:text-[var(--brand)] hover:bg-[var(--hover-bg)]"
                              title="编辑显示名称/目标地址"
                              onClick={() =>
                                setEditingLink({
                                  itemId: it.id,
                                  linkId: link.id,
                                  displayName: link.displayName,
                                  targetUri: link.targetUri,
                                  kind: link.kind,
                                })
                              }
                            >
                              ✏️
                            </button>
                            <button
                              className="w-5 h-5 flex items-center justify-center rounded-full text-[10px] text-[var(--t4)] hover:text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                              title="删除链接"
                              onClick={() => removeLink(it.id, link.id)}
                            >
                              🗑️
                            </button>
                          </div>
                        </div>
                      )
                    )}
                  </div>
                )}

                {/* 添加链接区：显示名称 + 目标地址双字段 */}
                {draftFor === it.id ? (
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <span className="text-[10px] text-[var(--t5)]">显示名称</span>
                    <input
                      autoFocus
                      value={draft.displayName}
                      placeholder="别名（选文件自动填文件名，可改）"
                      className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
                      onChange={(e) =>
                        setDraft({ ...draft, displayName: e.target.value })
                      }
                      onPointerDown={stop}
                    />
                    <span className="text-[10px] text-[var(--t5)]">目标地址</span>
                    <input
                      value={draft.targetUri}
                      placeholder="网址或路径"
                      className="nm-inset min-w-0 flex-1 rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
                      onChange={(e) => setDraft({ ...draft, targetUri: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.nativeEvent.isComposing) commitAddLink(it.id);
                      }}
                      onPointerDown={stop}
                    />
                    <button
                      className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t3)]"
                      title="选择文件（自动预填文件名到显示名称）"
                      onClick={() => pickLocal(false)}
                      onPointerDown={stop}
                    >
                      📄
                    </button>
                    <button
                      className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t3)]"
                      title="选择文件夹"
                      onClick={() => pickLocal(true)}
                      onPointerDown={stop}
                    >
                      📁
                    </button>
                    <button
                      className="nm-btn shrink-0 px-3 py-1 text-xs text-[var(--t3)]"
                      onClick={() => commitAddLink(it.id)}
                      onPointerDown={stop}
                    >
                      添加
                    </button>
                    <button
                      className="shrink-0 px-2 py-1 text-xs text-[var(--t5)]"
                      onClick={() => setDraftFor(null)}
                      onPointerDown={stop}
                    >
                      取消
                    </button>
                  </div>
                ) : (
                  <button
                    className="mt-2 text-xs text-[var(--brand)] hover:text-[var(--brand-strong)]"
                    onClick={() => openAddForm(it.id)}
                  >
                    ＋ 添加链接
                  </button>
                )}
              </div>
            )}
                  </div>
                )}
              </SortableWorkspaceItem>
            ))}
          </SortableContext>
        </DndContext>
      )}
    </div>
  );
}

/** 工作区可排序卡片：useSortable 注入拖拽能力，☰ 手柄 listeners 经 render prop 交给标题行 */
function SortableWorkspaceItem({
  it,
  children,
}: {
  it: WorkspaceItem;
  children: (listeners: DraggableSyntheticListeners | undefined) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: it.id });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      className={`group nm-card p-4 ${isDragging ? "opacity-70" : ""}`}
    >
      {children(listeners)}
    </div>
  );
}
