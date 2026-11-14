//! 工具重复调用闸：同一工具以完全相同参数连续调用达上限即拦截。
//!
//! 业界同构：Kilocode / hermes-agent 的 doom-loop detection——指纹 + 连击
//! 计数 + 第 3 次相同调用暂停。拦截形态照抄收尾窗先例（fuse.rs）：
//! role=tool 拒文回填 + continue，不计数、不进失败链（闸拒绝不是工具失败，
//! 叠一层 fail_hint 是双重提示）。
//!
//! 状态生命周期与 consec_failures 同（一次 run_model_loop_core，提问级）：
//! 跨轮累积、跨提问自然清零——新问题里重跑同一条命令是合法的新尝试。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 回合内重复闸状态。
pub(crate) struct RepeatGateState {
    /// 上一次受闸调用的指纹。
    last_fp: Option<u64>,
    /// 与 `last_fp` 相同的连续调用次数（1 起）。
    consec: usize,
    /// 本回合是否已转过问：问过（无论应答 / 超时 / 不可用）之后再撞闸一律
    /// 拒文——防「问完接着循环→再问」把转问本身变成新循环。
    ask_used: bool,
}

impl RepeatGateState {
    pub(crate) fn new() -> Self {
        Self {
            last_fp: None,
            consec: 0,
            ask_used: false,
        }
    }

    /// 受闸工具调用进门：更新连击并返回当前连击数（达上限应拦截）。
    ///
    /// 豁免工具不进来（调用方先用 `is_exempt_tool` 分流）——夹在重复链
    /// 中间的合法读不打断写链条的连击计数。
    pub(crate) fn observe(&mut self, name: &str, args: &str) -> usize {
        let fp = call_fingerprint(name, args);
        if self.last_fp == Some(fp) {
            self.consec += 1;
        } else {
            self.last_fp = Some(fp);
            self.consec = 1;
        }
        self.consec
    }

    /// 本回合还能不能转过问（每回合至多一次）。
    pub(crate) fn can_ask(&self) -> bool {
        !self.ask_used
    }

    /// 标记本回合已转过问（在发起征询前置位）。
    pub(crate) fn mark_ask_used(&mut self) {
        self.ask_used = true;
    }

    /// 用户应答后重置连击：答「继续再试一次」时重试必须放行。
    pub(crate) fn reset_tracking(&mut self) {
        self.last_fp = None;
        self.consec = 0;
    }
}

/// 豁免表命中与否：表内工具连续相同调用不计数（重查是合法语义）。
pub(crate) fn is_exempt_tool(tool: &str) -> bool {
    crate::bot::constants::REPEAT_GATE_EXEMPT_TOOLS.contains(&tool)
}

/// 调用指纹：工具名 + 规范化参数（JSON 递归按键排序）联合哈希。
///
/// 只在回合内比较，不要求跨进程稳定（DefaultHasher 足够）。参数解析失败
/// 按原文串哈希——闸拦的是「同样的调用」，原文相等也算同样。
pub(crate) fn call_fingerprint(name: &str, args: &str) -> u64 {
    let canonical = serde_json::from_str::<serde_json::Value>(args)
        .map(|v| canonical_json(&v))
        .unwrap_or_else(|_| args.to_string());
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    canonical.hash(&mut h);
    h.finish()
}

/// JSON 值递归按键排序序列化：键序不同的同参调用算同一次调用。
///
/// 不依赖 serde_json 的 Map 排序行为（`preserve_order` feature 一旦被某个
/// 依赖打开，Map 换成保序实现，「靠默认即有序」的假设就静默失效）。
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(k.clone()),
                        canonical_json(&map[k])
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
        serde_json::Value::Array(items) => {
            let inner = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{inner}]")
        }
        other => other.to_string(),
    }
}

/// 拒绝文案（不可提问分支）：点名连击数与禁令，给两条出路。
pub(crate) fn reject_text(name: &str, consec: usize) -> String {
    format!(
        "⛔ 死循环闸：工具 {name} 已连续 {consec} 次以完全相同的参数调用，本次未执行。请换一条路（改参数、换工具、把任务拆成更小的步骤），或按已有产出如实收尾；不得再以相同参数调用该工具。"
    )
}

/// 转问文案草稿：问题 / 缘由 / 选项 / 假设，全部在引擎参数红线内
/// （question ≤200 / why ≤100 / options ≤4×40 / assumption ≤200）。
pub(crate) struct RepeatAskDraft {
    pub(crate) question: String,
    pub(crate) why: String,
    pub(crate) options: Vec<String>,
    pub(crate) assumption: String,
}

/// 组装转问草稿：三选项 + 最保守的假设（无人应答时按「就此打住」收尾）。
pub(crate) fn ask_draft(name: &str, consec: usize) -> RepeatAskDraft {
    RepeatAskDraft {
        question: format!(
            "工具 {name} 已连续 {consec} 次以完全相同的参数调用，疑似原地循环，本次已暂停。怎么继续？"
        ),
        why: "同一调用连发无进展，死循环闸自动征询裁决".to_string(),
        options: vec![
            "换方法重试".to_string(),
            "继续再试一次".to_string(),
            "就此打住".to_string(),
        ],
        assumption: "就此打住：保留已有产出，停止该方向的继续尝试并收尾".to_string(),
    }
}

/// 应答回填文案：用户裁决原样透传，模型据此继续。
pub(crate) fn answered_text(name: &str, consec: usize, verdict: &str) -> String {
    format!(
        "⛔ 死循环闸：工具 {name} 第 {consec} 次相同调用已拦截未执行。已征询用户，裁决：{verdict}。请按裁决继续。"
    )
}

/// 转问未获应答的回落文案：与拒文同向（换路或收尾），出路不变。
pub(crate) fn fallback_text(name: &str, consec: usize) -> String {
    format!(
        "⛔ 死循环闸：工具 {name} 已连续 {consec} 次以完全相同的参数调用，本次未执行；转问用户未获应答（无人值守、提问预算已尽或超时）。请换一条路（改参数、换工具、把任务拆成更小的步骤），或按已有产出如实收尾；不得再以相同参数调用该工具。"
    )
}
