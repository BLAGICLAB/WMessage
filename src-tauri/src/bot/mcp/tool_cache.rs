//! MCP 工具清单的磁盘快照（按需启停的「工具可见性」支撑）。
//!
//! 按需启停之后，外部 MCP 服务器平时**不连着**——但模型要看得见它的工具
//! 才能调用它。「连着才看得见」与「看得见才连」互为死锁，必须把两件事拆开：
//! **工具元数据与活连接解耦**。
//!
//! 业界同款做法（调研见 `docs/MCP-ON-DEMAND-LIFECYCLE-DESIGN-2026-10-24.md`）：
//! - mcp-proxy 的 lazy 生命周期：`tools/list` 触发一次连接，发现完即断，
//!   之后工具照样可见；`tools/call` 再透明重连；
//! - VS Code 的 `chat.mcp.autostart`：按需启动，工具清单与进程存活解耦；
//! - rho 引擎：`lifecycle: "lazy"` + `mcp-cache.json` 元数据缓存。
//!
//! 本模块就是那本 `mcp-cache.json`：按服务器 id 记「配置指纹 + 工具清单」。
//!
//! 失效判据 = **配置指纹**（`manager::fingerprint`）：改了命令 / 参数 / env /
//! url / 超时 → 指纹变 → 快照作废 → 重新发现。指纹认 env/headers（机密字段
//! `skip_serializing` 剥掉了序列化值，用序列化值算就看不见它们）。
//!
//! 落盘内容是第三方服务器给的工具名 / 描述 / inputSchema（**外部数据，非机密**）；
//! 不含 env / headers 等机密，也不含任何凭据。

use std::collections::HashMap;
use std::path::Path;

use rmcp::model::Tool;
use serde::{Deserialize, Serialize};

/// 快照文件名（数据目录下）
const FILE_NAME: &str = "mcp-tool-cache.json";

/// 单台服务器的工具快照
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// 配置指纹：配置一变快照即失效（认 env/headers，见模块头）
    pub fingerprint: u64,
    /// 服务器显示名（诊断用；工具名前缀另由 `config::mcp_tool_name` 派生）
    pub name: String,
    /// 工具清单（可能为空 = 服务器确实没提供工具）
    pub tools: Vec<Tool>,
}

/// 全量快照（按服务器 id 索引）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCache {
    #[serde(default)]
    pub servers: HashMap<String, Entry>,
}

impl ToolCache {
    /// 读快照：文件不存在 / 内容坏掉 → 空快照（fail-soft）。
    ///
    /// 坏快照不报错：最坏后果是「这次会话多连一次服务器重新发现」，
    /// 而报错会让一个陈旧文件卡死整条 MCP 通路。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        // 坏 JSON / 旧版 rmcp 的工具结构 → 当空快照处理（重新发现即可）
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// 落盘（tmp + rename：崩溃不留半截文件）。
    ///
    /// 半截文件的后果不是「读不到」而是「读到一半」——那会让下一次启动把
    /// 残缺的工具清单当成真清单挂给模型（少了工具还以为是服务器给的）。
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        if std::fs::create_dir_all(dir).is_err() {
            return Err(format!("数据目录不可建：{}", dir.display()));
        }
        let path = dir.join(FILE_NAME);
        let raw =
            serde_json::to_string_pretty(self).map_err(|e| format!("工具快照序列化失败：{e}"))?;
        let tmp = dir.join(format!("{}.{}.tmp", FILE_NAME, std::process::id()));
        std::fs::write(&tmp, raw.as_bytes()).map_err(|e| format!("工具快照写入失败：{e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            // rename 失败要清掉 tmp 残渣（下次写会撞同名）
            let _ = std::fs::remove_file(&tmp);
            format!("工具快照落盘失败：{e}")
        })
    }

    /// 取某台服务器的快照：**指纹一致才算数**（配置变了这份就是陈旧的）。
    pub fn entry(&self, id: &str, fingerprint: u64) -> Option<&Entry> {
        self.servers
            .get(id)
            .filter(|e| e.fingerprint == fingerprint)
    }

    /// 写入某台服务器的快照（覆盖同 id 的旧条目）
    pub fn put(&mut self, id: &str, fingerprint: u64, name: &str, tools: Vec<Tool>) {
        self.servers.insert(
            id.to_string(),
            Entry {
                fingerprint,
                name: name.to_string(),
                tools,
            },
        );
    }

    /// 移除某台服务器的快照（服务器被删除时调用；留着也没用——按 id 找不到配置）
    pub fn remove(&mut self, id: &str) {
        self.servers.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Tool {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "description": "测试工具",
            "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}}
        }))
        .expect("工具应能反序列化")
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wmessage-toolcache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录应可建");
        dir
    }

    #[test]
    fn roundtrip_preserves_tool_names_and_schemas() {
        let dir = tmpdir("roundtrip");
        let mut cache = ToolCache::default();
        cache.put("srv-1", 42, "fs", vec![tool("read"), tool("write")]);
        cache.save(&dir).expect("快照应能落盘");

        let back = ToolCache::load(&dir);
        let e = back.entry("srv-1", 42).expect("指纹一致应命中");
        assert_eq!(e.name, "fs");
        assert_eq!(e.tools.len(), 2);
        assert_eq!(e.tools[0].name, "read");
        // inputSchema 原样回来（参数 schema 丢了模型就调不对）
        let params = e.tools[0]
            .input_schema
            .get("properties")
            .expect("参数 schema 应在");
        assert!(params.get("x").is_some(), "参数 schema 丢了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_mismatch_invalidates_the_snapshot() {
        let dir = tmpdir("fingerprint");
        let mut cache = ToolCache::default();
        cache.put("srv-1", 42, "fs", vec![tool("read")]);
        cache.save(&dir).expect("快照应能落盘");

        let back = ToolCache::load(&dir);
        assert!(back.entry("srv-1", 42).is_some(), "同指纹应命中");
        assert!(
            back.entry("srv-1", 43).is_none(),
            "配置变了（指纹不同）快照必须失效——否则挂给模型的是旧工具清单"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_broken_file_yields_empty_cache() {
        let dir = tmpdir("broken");
        assert!(
            ToolCache::load(&dir).servers.is_empty(),
            "文件不存在 → 空快照"
        );
        std::fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert!(
            ToolCache::load(&dir).servers.is_empty(),
            "坏文件 → 空快照（fail-soft：最坏多连一次服务器重新发现）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_tmp_residue() {
        let dir = tmpdir("residue");
        let cache = ToolCache::default();
        cache.save(&dir).expect("空快照也应能落盘");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不得遗留 tmp 残渣");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
