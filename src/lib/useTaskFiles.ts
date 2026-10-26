import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Task } from "../types";
import { handleCommandError } from "./errorHandler";
import { applyBinding, taskFiles, type TaskFile } from "./taskFiles";

/**
 * 任务卡文件绑定（统一通道）。
 *
 * 此前有**三份**绑定实现，且已经漂移：卡片侧（TodoCard）持有 taskRef，
 * 合并基线取「对话框返回后的现值」；详情面板侧（TaskDetailPanel）与上下文栏侧
 * （ContextRail）用渲染期快照 —— 用户在这两处开着的同时另一路径（卡片 / 详情
 * 面板）绑定文件，那批改动会被整列覆盖**丢失**，而详情面板的注释还自称
 * 「与 TodoCard 同通道」。
 *
 * 现在只有这一份：合并基线一律取「await 之后的最新 files」。写侧差异
 * （TodoCard 走 onUpdate(taskId, patch)，详情面板走 onUpdate(patch)）由调用方
 * 用 `commit` 回调表达。选文件通道也统一走 Rust 原生对话框
 * （pick_files_dialog，与 ContextRail 同一条），不再用 webview dialog。
 */
export function useTaskFiles(
  task: Task,
  /** 合并后的新绑定落库（调用方决定写入通道） */
  commit: (files: TaskFile[]) => void,
) {
  // 最新 task 镜像：绑定对话框挂起期间 files 可能被别处改动（另一窗口/机器人），
  // 合并基线必须取对话框返回后的现值，而非点击时的快照（ref 写放 effect）
  const taskRef = useRef(task);
  useEffect(() => {
    taskRef.current = task;
  });
  // commit 走 ref：调用方多半传内联箭头，按它做 useCallback 依赖会让三个
  // 回调每帧换身份（memo 子组件白 memo）
  const commitRef = useRef(commit);
  useEffect(() => {
    commitRef.current = commit;
  });

  const pickFile = useCallback(async () => {
    try {
      // 文件与文件夹不互斥：已绑文件夹也可继续添加文件
      const paths = await invoke<string[]>("pick_files_dialog");
      if (paths.length === 0) return;
      // isDir 由 Rust 侧 fs::metadata 判定（前端无法 stat）
      const added = await invoke<TaskFile[]>("bind_files", { paths });
      // await 期间可能有并发绑定：以最新 files 为合并基线，别丢掉那批改动
      applyBinding(taskFiles(taskRef.current), added, (files) =>
        commitRef.current(files),
      );
    } catch (e) {
      handleCommandError(e, "pick file");
    }
  }, []);

  const pickFolder = useCallback(async () => {
    try {
      // 文件夹单选（每卡一个文件夹槽）；文件与文件夹不互斥。
      // 绑定一律加法：已有文件夹时新文件夹替换旧文件夹（占原位），不再拦截
      const selected = await invoke<string[]>("pick_files_dialog", {
        directory: true,
      });
      const path = selected[0];
      if (!path) return;
      applyBinding(
        taskFiles(taskRef.current),
        [{ path, isDir: true }],
        (files) => commitRef.current(files),
      );
    } catch (e) {
      handleCommandError(e, "pick folder");
    }
  }, []);

  const removeFile = useCallback((path: string) => {
    commitRef.current(
      taskFiles(taskRef.current).filter((f) => f.path !== path),
    );
  }, []);

  return { pickFile, pickFolder, removeFile };
}
