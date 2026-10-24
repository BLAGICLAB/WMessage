//! MCP（Model Context Protocol）宿主支持：wmessage 作为 Host/Client 连接外部 MCP 服务器。
//!
//! - `config`：McpServerConfig 数据结构 + 校验（含启动器白名单）+ 工具命名（纯逻辑）
//! - `builtin`：内置条目（Playwright 浏览器操控）的目录解析 + 启动时补写配置
//! - `commands`：设置页 CRUD 命令（保存/删除/启停/状态，窄口径 RMW 写 bot-config.json）
//! - `manager`：连接生命周期（按需启停：静默待命 / 调用期拉起 / 调用完回收 / 懒重连）
//! - `tool_cache`：工具清单的磁盘快照（按需启停下「工具可见性」与「活连接」解耦）
//! - `stdio_transport`：stdio 子进程传输（读侧行长上限 + 子进程生命周期托管）
//!
//! - `secrets`： 机密存储（env/headers 走 keyring/降级文件，配置永不明文）
//!
//! 边界：只做宿主侧——连接**外部** MCP 服务器；
//! 不做 wmessage 自身作为 MCP Server 对外暴露（另开任务）。
//! 接入方式：外部工具是「增量挂载」——不改既有内置工具（registry::BUILTIN_TOOL_COUNT 个）的注册机制，
//! 工具发现/注入/调用路由见 registry / dispatch 的 mcp 钩子（阶段 3/4）。

pub mod builtin;
pub mod commands;
pub mod config;
pub mod manager;
pub mod mount;
pub mod secrets;
pub mod stdio_transport;
pub mod tool_cache;
