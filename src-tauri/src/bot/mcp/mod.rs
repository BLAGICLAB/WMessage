//! MCP（Model Context Protocol）宿主支持：wmessage 作为 Host/Client 连接外部 MCP 服务器。
//!
//! - `config`：McpServerConfig 数据结构 + 校验（含启动器白名单）+ 工具命名（纯逻辑）
//! - `commands`：设置页 CRUD 命令（保存/删除/启停/状态，窄口径 RMW 写 bot-config.json）
//! - `manager`：连接生命周期（连/断/指纹对齐重载/懒重连/调用，McpManager 托管状态）
//!
//! - `secrets`：B4-6 机密存储（env/headers 走 keyring/降级文件，配置永不明文）
//!
//! 边界（老板 2026-09-28 拍板）：只做宿主侧——连接**外部** MCP 服务器；
//! 不做 wmessage 自身作为 MCP Server 对外暴露（另开任务）。
//! 接入方式：外部工具是「增量挂载」——不改既有 29+3 内置工具的注册机制，
//! 工具发现/注入/调用路由见 registry / dispatch 的 mcp 钩子（阶段 3/4）。

pub mod commands;
pub mod config;
pub mod manager;
pub mod mount;
pub mod secrets;
