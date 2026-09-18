//! R5 决策面板 · 后端 Tauri commands 模块
//!
//! spec R5：
//! - 列出 Pending 提案（evidence / diff / 影响）
//! - 操作：Promote / Reject / Keep Shadow
//! - 复用 ConfirmMap 确认弹窗
//! - 回滚按钮（开发者视角）
//!
//! 验收：
//! - 端到端：候选 → 用户批准 → 生效（apply 集成在 R4 之后由 apply_from_consolidation 接入）
//! - 端到端：用户回滚 → 下轮 injection_block 不再包含（删 mem_item）

pub mod commands;