//! 公共返回类型：API 启用信息 / 状态。

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiInfo {
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiStatus {
    pub enabled: bool,
    pub port: u16,
    /// disabled 时为 None,字段在序列化输出里直接缺席(避免泄露空串,
    /// 也避免任何 caller 不分 enabled 状态都拿到字段)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}
