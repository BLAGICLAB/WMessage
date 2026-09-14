//! 公共返回类型：API 启用信息 / 状态。

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiInfo {
    pub port: u16,
    pub token: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiStatus {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
}
