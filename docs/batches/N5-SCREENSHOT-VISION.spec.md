# Batch Spec: N5-SCREENSHOT-VISION screenshot 直达模型视觉（去掉 OCR 中转）

```json
{
  "batch_id": "N5-SCREENSHOT-VISION",
  "family": "bot-tools",
  "expected_files": [
    "docs/batches/N5-SCREENSHOT-VISION.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_desktop.rs",
    "src-tauri/src/bot_anthropic.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/tests/llm_integration.rs"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 30,
  "findings": [
    {"id": "N5-1", "file": "src-tauri/src/bot/registry.rs", "line": 52, "fix": "ToolResult 加 images: Vec<String>（绝对路径，默认空）+ ok_with_images 构造器；其余 36 工具不受影响"},
    {"id": "N5-2", "file": "src-tauri/src/bot_chat.rs", "line": 440, "fix": "抽 pub(crate) image_part_from_file：图片文件 → image_url data-URL part（同上限 3MB/同 mime 表，无白名单——调用方是工具自身产物）"},
    {"id": "N5-3", "file": "src-tauri/src/bot_model_loop.rs", "line": 1390, "fix": "回填点：tool 消息照推后，images 非空 → 追加 user 消息 [〔系统附图〕text + image_url parts]（OpenAI 协议 tool 只收文本，官方视觉示例同款「工具后追加带图 user 消息」）；读取失败逐图跳过并审计"},
    {"id": "N5-4", "file": "src-tauri/src/bot_desktop.rs", "line": 240, "fix": "screenshot 改 ok_with_images——图随 ToolResult 直达模型视觉，不再引导 OCR 中转"},
    {"id": "N5-5", "file": "src-tauri/src/bot_anthropic.rs", "line": 53, "fix": "零改动验证：convert_content_blocks 已转 data-URL image 块 + flush/push_or_merge 把 [tool, user(图)] 合并单条 user [tool_result, image]（官方形态）——新增合并断言测试锁死"},
    {"id": "N5-6", "file": "src-tauri/src/prompts/system.rs", "line": 22, "fix": "规则 23 与 SCHEMA_SCREENSHOT 文案：截图直接附给模型用视觉读取，去掉 ocr 中转描述（ocr_image 工具本身保留）"}
  ],
  "assertions_min": {
    "src-tauri/tests/llm_integration.rs": 1,
    "src-tauri/src/bot_anthropic.rs": 1
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

老板指出：现在的模型都有视觉，screenshot 走 OCR 中转多余。打通「工具返回图片
直达模型视觉」链路：ToolResult 携带图片路径 → 模型循环注入对话（OpenAI 协议
追加带图 user 消息 / Anthropic 协议 tool_result 内嵌图）。

## 协议依据

- OpenAI：tool 消息 content 官方只收文本；`assistant(tool_calls) → tool →
  user(image_url parts)` 序列合法（不要求严格交替），官方视觉示例同款。
- Anthropic：tool_result 块官方支持附图；本仓转换器 flush/push_or_merge 已
  把 [tool, user(图)] 合并单条 user（严格交替满足），本批加测试锁死。

## 出界

- 其他工具不加图返回（按需后续）；ocr_image 保留（磁盘任意图片文字提取）；
- 图片消息不做历史裁剪（循环内 msgs 生命周期单次运行，注释留档）；
- 前端零改动（图仅进模型上下文，不进聊天 UI）。
