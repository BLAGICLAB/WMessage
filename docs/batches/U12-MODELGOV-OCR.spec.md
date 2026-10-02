# Batch Spec: U12-MODELGOV-OCR

## 目的

U12 提交后补跑 ocr review（`docs/OCR-CODE-REVIEW-2026-10-02-u12.json`，75 条：
1C/6H/22M/46L）的处置批。

**处置**：

- **critical 驳回**（误报）：声称 lobehub 无 minimax 资产、别名表是占位符——实测
  `src/assets/providers/lobe/minimax-color.svg` 存在，ProviderLogo 用例绿，证伪。
- **high 修 5**：①`probe_connection` 剥误粘端点尾段（/v1/messages、/messages、
  /chat/completions），与 anthropic_messages_url 同规则 + 集成用例；②`read_llm_key`
  厂商 keyring 真实故障上抛（不再静默回落全局 key 拿错 key 打 401）；③
  `bot_test_connection` 厂商 key 读取同款改 has→Err 上抛；④meta 同步响应体 16MB
  上限（防劫持/异常响应打爆内存）；⑤normalizeVendorName 循环剥「括号+套餐后缀」
  到稳定（两种先后组合都覆盖）。
- **medium 修 9**：厂商 key CRUD 空名守卫（InvalidArgument）；delete_vendor_key
  补降级告警；bot_clear_vendor_key 改先落盘后删 key（fail-closed）；probe 写盘
  失败从 eprintln 升级为审计；meta_model(provider_key) 加索引；meta 四个写函数
  补 db 写锁 debug_assert 契约（测试持锁）；sync_models_dev_with_url 收
  pub(crate)；ModelRow 插头状态：URL/协议变更重置瞬态 + testing 态显灰 + 嵌套
  三元改映射；MetaSyncResult 收紧为判别联合。
- **medium 驳回/登记**：vitest.config.ts 的 build.assetsInlineLimit 实测是
  必需的（移除后 ?url 返回空串，logo 断言全挂，已回退）；read_llm_key 直测
  需 keyring 注入重构（登记）；probe 读 key 在锁外（毫秒窗口，登记）；meta
  同步测试共享写锁（现有测试全绿，登记）。
- low 46 条：逐条过目，均为风格/注释级，登记不修。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U12-MODELGOV-OCR",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U12-MODELGOV-OCR.spec.md",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/keyring.rs",
    "src-tauri/src/db/migrations.rs",
    "src-tauri/src/meta/mod.rs",
    "src-tauri/src/meta/sync.rs",
    "src-tauri/tests/bot_test_connection.rs",
    "src/components/SettingsPage/ModelRow.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/providerLogoMap.ts",
    "src/lib/modelMeta.ts",
    "vitest.config.ts"
  ],
  "max_lines_added": 200,
  "max_lines_removed": 80,
  "max_new_files_lines": 120,
  "findings": [
    { "file": "src-tauri/src/bot/config/keyring.rs", "note": "read_llm_key 直测需 keyring 后端注入重构，登记" },
    { "file": "src-tauri/src/meta/sync.rs", "note": "meta 同步测试共享进程级写锁，登记" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh    # exit 0
```
