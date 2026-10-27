// ChatPanel 系测试共享的 Tauri invoke 默认返回值表。
// 只收编「默认实现」这一复制大头：vi.fn() 的创建必须留在各测试文件的
// vi.hoisted 里（提升先于 import 求值，共享模块帮不上），本模块只承载
// 14 分支 switch。用例要改某命令返回值时整体 mockImplementation 覆盖，
// 测完用本表恢复（beforeEach 只 mockClear 不重置 implementation 的约定）。

/** ChatPanel 测试基线：默认返回值表（未登记命令一律 null） */
export const chatDefaultInvoke = async (cmd: string) => {
  switch (cmd) {
    case "bot_sessions_load":
      return [{ id: "s1", title: "默认会话" }];
    case "bot_session_create":
      return { id: "s-new", title: "新对话" };
    case "bot_history_load":
      return [];
    case "bot_history_save":
    case "bot_history_clear":
    case "bot_session_delete":
    case "bot_session_rename":
      return null;
    case "bot_chat":
      return { text: "机器人回复", taskRefs: [] };
    // 双命令：🧠 模型下拉的列表 + 窄口径切换
    case "bot_get_config":
      return {
        baseUrl: "https://api.example.com/v1",
        model: "MiniMax-M3",
        apiProvider: "openai",
        modelsByProvider: {
          openai: [
            {
              id: "m1",
              label: "MiniMax-M3",
              baseUrl: "https://api.example.com/v1",
              model: "MiniMax-M3",
            },
            {
              id: "m2",
              label: "DeepSeek-V3",
              baseUrl: "https://api.deepseek.com",
              model: "deepseek-chat",
            },
          ],
          anthropic: [],
        },
        activeModelId: { openai: "m1", anthropic: null },
      };
    case "bot_set_active_model":
      return {
        model: "deepseek-chat",
        apiProvider: "openai",
        activeModelId: { openai: "m2", anthropic: null },
      };
    case "bot_execute_task":
      return { text: "任务执行结果", taskRefs: [] };
    case "bot_compact":
      return "压缩后的摘要";
    case "bot_stop":
      return null;
    case "pick_files_dialog":
      return [];
    default:
      return null;
  }
};
