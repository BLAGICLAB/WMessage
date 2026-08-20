use once_cell::sync::Lazy;
use regex::Regex;

// ─────────────────────── 变量替换（Phase 2 2026-08-17 23:23）───────────────────────

/// 任务卡 UUID 提取正则（标准 UUID v4 格式）
static TASK_ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
});

/// `${stepN.field}` 索引匹配（field ∈ {result, id}）
static VAR_BY_INDEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$\{step(\d+)\.(result|id)\}").unwrap());

/// `${prev.field}` 上一步简写
static VAR_PREV: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{prev\.(result|id)\}").unwrap());

/// `${stepN.path.to.field}` 嵌套路径（Phase 4 第 2 项 2026-08-18 06:25）
/// 路径 ≥2 段（首段标识符 + 后续 `.xxx`），与单段 result/id 不冲突
/// 例：`${step1.task.id}` / `${step1.list.0.title}` / `${step1.a.b.c.d}`
static VAR_NESTED_BY_INDEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$\{step(\d+)\.([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*)\}").unwrap());

/// `${prev.path.to.field}` 嵌套路径简写（同样 ≥2 段）
static VAR_NESTED_PREV: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$\{prev\.([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*)\}").unwrap());

/// 已完成步骤的快照（变量替换上下文，调度器维护）
#[derive(Debug, Clone)]
pub struct CompletedStep {
    pub index: usize,
    /// 步骤标题。substitute_vars 不读，仅作为 ctx roundtrip 快照保留
    /// （调试 / 未来审计 / Skill 跨步 context 用）
    #[allow(dead_code)]
    pub title: String,
    /// 工具返回的原始文本
    pub result: String,
    /// 从 result 提取的 task id（UUID）；没找到为 None
    pub id: Option<String>,
    /// JSON 解析结果（execute_tool 返回字符串尝试 parse_json，失败 None）—— 嵌套路径用
    /// Phase 4 第 2 项 2026-08-18 06:25：substitute_vars 走 serde_json::Value 路径取值
    pub parsed: Option<serde_json::Value>,
}

/// 从工具结果文本中提取第一个 UUID；找不到返回 None。
pub fn extract_task_id(text: &str) -> Option<String> {
    TASK_ID_RE.find(text).map(|m| m.as_str().to_string())
}

/// 变量替换（Phase 2 核心）：
/// - `${stepN.result}` → 第 N 步的工具返回文本
/// - `${stepN.id}` → 第 N 步从结果提取的 UUID（任务卡专用）
/// - `${prev.result}` → 上一步工具返回文本
/// - `${prev.id}` → 上一步提取的 UUID
/// - 未匹配的 `${...}` 保留原样（避免误吃合法 JSON 里的 `$` 字符）
pub fn substitute_vars(text: &str, ctx: &[CompletedStep]) -> String {
    // Phase 4 第 2 项（2026-08-18 06:25）：嵌套路径优先匹配
    // （`${stepN.task.id}` / `${prev.list.0.title}` / `${stepN.a.b.c.d}`）
    // 路径 ≥2 段才走嵌套 regex，单段 result/id 留给下方 VAR_BY_INDEX / VAR_PREV 处理
    let r0 = VAR_NESTED_BY_INDEX.replace_all(text, |caps: &regex::Captures| {
        let idx: usize = match caps[1].parse() {
            Ok(n) => n,
            Err(_) => return caps[0].to_string(),
        };
        let path = &caps[2];
        // 单段 result/id 让 VAR_BY_INDEX 后续处理（向后兼容）
        if !path.contains('.') && (path == "result" || path == "id") {
            return caps[0].to_string();
        }
        match ctx.iter().find(|s| s.index == idx) {
            Some(step) => resolve_nested_path(step.parsed.as_ref(), path)
                .unwrap_or_else(|| caps[0].to_string()),
            None => caps[0].to_string(),
        }
    });
    let r1 = VAR_NESTED_PREV.replace_all(&r0, |caps: &regex::Captures| {
        let path = &caps[1];
        // 单段 result/id 让 VAR_PREV 后续处理（向后兼容）
        if !path.contains('.') && (path == "result" || path == "id") {
            return caps[0].to_string();
        }
        match ctx.last() {
            Some(last) => resolve_nested_path(last.parsed.as_ref(), path)
                .unwrap_or_else(|| caps[0].to_string()),
            None => caps[0].to_string(),
        }
    });
    let r2 = VAR_BY_INDEX.replace_all(&r1, |caps: &regex::Captures| {
        let idx: usize = match caps[1].parse() {
            Ok(n) => n,
            Err(_) => return caps[0].to_string(),
        };
        let field = &caps[2];
        match ctx.iter().find(|s| s.index == idx) {
            Some(step) => match field {
                "result" => step.result.clone(),
                "id" => step.id.clone().unwrap_or_default(),
                _ => caps[0].to_string(),
            },
            None => caps[0].to_string(),
        }
    });
    let r3 = VAR_PREV.replace_all(&r2, |caps: &regex::Captures| {
        let field = &caps[1];
        match ctx.last() {
            Some(last) => match field {
                "result" => last.result.clone(),
                "id" => last.id.clone().unwrap_or_default(),
                _ => caps[0].to_string(),
            },
            None => caps[0].to_string(),
        }
    });
    r3.into_owned()
}

/// JSON 路径解析（Phase 4 第 2 项 2026-08-18 06:25）：沿 serde_json::Value 走路径取值
/// - 数字段 → 数组索引（`usize` 解析）
/// - 非数字段 → 对象字段
/// - 字段不存在 / parsed 为 None / 数组越界 → 返回 None（调用方决定 fallback：保留 `${...}`）
/// - 终值是 String → 原样；Null → "null"；其他（数字/布尔/数组/对象）→ serde_json 序列化成字符串
fn resolve_nested_path(parsed: Option<&serde_json::Value>, path: &str) -> Option<String> {
    let mut current = parsed?;
    for segment in path.split('.') {
        current = if let Ok(idx) = segment.parse::<usize>() {
            current.as_array()?.get(idx)?
        } else {
            current.as_object()?.get(segment)?
        };
    }
    Some(match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    // ── Phase 2 变量替换（2026-08-18 05:18 接入 run_skill_scheduler） ──

    fn ctx_one_step(uuid: &str, result: &str) -> Vec<CompletedStep> {
        vec![CompletedStep {
            index: 1,
            title: "step-1".into(),
            result: result.into(),
            id: Some(uuid.into()),
            // Phase 4 第 2 项：result 尝试 parse_json，失败 None（嵌套路径 fallback 保留原样）
            parsed: serde_json::from_str(result).ok(),
        }]
    }

    #[test]
    fn substitute_vars_resolves_step_index_id() {
        // 任务卡 UUID 跨步骤传递：${step1.id} → 真实 UUID
        let ctx = ctx_one_step("7c9e6679-7425-40de-944b-e07fc1f90ae7", "task created");
        let args = r#"{"id": "${step1.id}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#
        );
    }

    #[test]
    fn substitute_vars_resolves_step_index_result() {
        // 步骤原始结果跨步骤传递：${step1.result} → 上一步工具返回原文
        // ctx.result 是 Rust String，\n 是真实换行符 0x0A（不是字面两字符 \n）
        let ctx = ctx_one_step("uuid-1", "第一行\n第二行");
        let args = "{\"note\": \"${step1.result}\"}";
        // 期望值同样含真实换行符
        assert_eq!(
            substitute_vars(args, &ctx),
            "{\"note\": \"第一行\n第二行\"}"
        );
    }

    #[test]
    fn substitute_vars_resolves_prev_alias() {
        // ${prev.*} 简写指向 ctx 最后一项
        let ctx = vec![
            CompletedStep {
                index: 1,
                title: "a".into(),
                result: "first".into(),
                id: None,
                parsed: None,
            },
            CompletedStep {
                index: 2,
                title: "b".into(),
                result: "second".into(),
                id: Some("uuid-2".into()),
                parsed: None,
            },
        ];
        assert_eq!(substitute_vars("${prev.result}", &ctx), "second");
        assert_eq!(substitute_vars("${prev.id}", &ctx), "uuid-2");
        // 多次出现的 ${prev.result} 全部替换
        assert_eq!(
            substitute_vars("[${prev.result}]-${prev.result}", &ctx),
            "[second]-second"
        );
    }

    #[test]
    fn substitute_vars_keeps_unknown_intact() {
        // 未匹配的 ${...} 保留原样（避免误吃合法 JSON 中的 $ 字符）
        let ctx = ctx_one_step("uuid-1", "r");
        // 越界索引：保留
        assert_eq!(
            substitute_vars(r#"{"ref": "${step99.id}"}"#, &ctx),
            r#"{"ref": "${step99.id}"}"#
        );
        // 非法 field：保留
        assert_eq!(
            substitute_vars(r#"{"x": "${step1.unknown}"}"#, &ctx),
            r#"{"x": "${step1.unknown}"}"#
        );
        // 合法 JSON 里的 $1.50 不应被吃
        assert_eq!(
            substitute_vars(r#"{"price": "$1.50"}"#, &ctx),
            r#"{"price": "$1.50"}"#
        );
        // 空 ctx：${prev.*} 保留
        assert_eq!(substitute_vars("${prev.id}", &[]), "${prev.id}");
    }

    #[test]
    fn extract_task_id_picks_first_uuid() {
        // 工具结果含任务卡 UUID → 提取
        let text = "任务已创建：\nID = 7c9e6679-7425-40de-944b-e07fc1f90ae7\n标题：买牛奶";
        assert_eq!(
            extract_task_id(text).as_deref(),
            Some("7c9e6679-7425-40de-944b-e07fc1f90ae7")
        );
        // 多 UUID 取首个
        let text2 =
            "first 11111111-2222-3333-4444-555555555555 then 66666666-7777-8888-9999-000000000000";
        assert_eq!(
            extract_task_id(text2).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        // 无 UUID → None
        assert_eq!(extract_task_id("no uuid here"), None);
        // 部分 UUID 格式（不足 36 字符）不应匹配
        assert_eq!(extract_task_id("short 7c9e6679-7425-40de-944b"), None);
    }

    #[test]
    fn substitute_vars_handles_multiple_indexes_in_one_call() {
        // 一个 args_json 里同时引用多个 step：${step1.id} + ${step2.result}
        let ctx = vec![
            CompletedStep {
                index: 1,
                title: "create".into(),
                result: "r1".into(),
                id: Some("uuid-1".into()),
                parsed: None,
            },
            CompletedStep {
                index: 2,
                title: "query".into(),
                result: "second-step-output".into(),
                id: None,
                parsed: None,
            },
        ];
        let args = r#"{"task": "${step1.id}", "note": "${step2.result}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"task": "uuid-1", "note": "second-step-output"}"#
        );
    }

    // ── Phase 4 第 2 项：嵌套路径（2026-08-18 06:25） ──

    fn ctx_one_step_parsed(uuid: &str, parsed_json: &str) -> Vec<CompletedStep> {
        vec![CompletedStep {
            index: 1,
            title: "step-1".into(),
            result: parsed_json.into(),
            id: Some(uuid.into()),
            parsed: serde_json::from_str(parsed_json).ok(),
        }]
    }

    #[test]
    fn nested_path_resolves_step_index_nested_field() {
        // `${step1.task.id}` 嵌套路径解析
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"task": {"id": "task-uuid-aaa", "title": "买牛奶"}}"#,
        );
        let args = r#"{"ref": "${step1.task.id}"}"#;
        assert_eq!(substitute_vars(args, &ctx), r#"{"ref": "task-uuid-aaa"}"#);
    }

    #[test]
    fn nested_path_resolves_prev_nested_field() {
        // `${prev.task.title}` 上一步嵌套字段
        let ctx = ctx_one_step_parsed("uuid-1", r#"{"task": {"title": "归档演示"}}"#);
        let args = r#"{"title": "${prev.task.title}"}"#;
        assert_eq!(substitute_vars(args, &ctx), r#"{"title": "归档演示"}"#);
    }

    #[test]
    fn nested_path_resolves_array_index() {
        // `${step1.1.id}` 数字段作为数组索引
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"[{"id": "first"}, {"id": "second"}, {"id": "third"}]"#,
        );
        let args = r#"{"id": "${step1.1.id}"}"#;
        assert_eq!(substitute_vars(args, &ctx), r#"{"id": "second"}"#);
    }

    #[test]
    fn nested_path_keeps_intact_when_field_missing() {
        // JSON 存在但字段缺失 → 保留 `${step1...}` 原样
        let ctx = ctx_one_step_parsed("uuid-1", r#"{"task": {"id": "x"}}"#);
        let args = r#"{"x": "${step1.task.title}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"x": "${step1.task.title}"}"#
        );
    }

    #[test]
    fn nested_path_keeps_intact_when_not_json() {
        // parsed = None（纯文本 result，JSON parse 失败）→ 保留原样
        let ctx = ctx_one_step("uuid-1", "plain text response");
        assert!(ctx[0].parsed.is_none());
        let args = r#"{"x": "${step1.task.id}"}"#;
        assert_eq!(substitute_vars(args, &ctx), r#"{"x": "${step1.task.id}"}"#);
    }

    #[test]
    fn nested_path_resolves_deep_chain() {
        // `${step1.a.b.c.d}` 深嵌套 5 层
        let ctx = ctx_one_step_parsed("uuid-1", r#"{"a": {"b": {"c": {"d": "deep-value"}}}}"#);
        let args = r#"{"x": "${step1.a.b.c.d}"}"#;
        assert_eq!(substitute_vars(args, &ctx), r#"{"x": "deep-value"}"#);
    }

    #[test]
    fn nested_path_serializes_non_string_value() {
        // 终值非字符串（数字/布尔/null）→ 序列化成字符串
        let ctx = ctx_one_step_parsed("uuid-1", r#"{"n": 42, "b": true, "z": null}"#);
        assert_eq!(
            substitute_vars(r#"{"v": "${step1.n}"}"#, &ctx),
            r#"{"v": "42"}"#
        );
        assert_eq!(
            substitute_vars(r#"{"v": "${step1.b}"}"#, &ctx),
            r#"{"v": "true"}"#
        );
        assert_eq!(
            substitute_vars(r#"{"v": "${step1.z}"}"#, &ctx),
            r#"{"v": "null"}"#
        );
    }

    #[test]
    fn nested_path_does_not_collide_with_single_segment_result_id() {
        // 回归测试：`${stepN.result}` / `${stepN.id}` 仍走原 VAR_BY_INDEX 路径
        // 不被嵌套 regex 抢先吃掉
        let ctx = ctx_one_step("task-uuid", "raw text result");
        assert_eq!(
            substitute_vars(r#"{"r": "${step1.result}"}"#, &ctx),
            r#"{"r": "raw text result"}"#
        );
        assert_eq!(
            substitute_vars(r#"{"i": "${step1.id}"}"#, &ctx),
            r#"{"i": "task-uuid"}"#
        );
    }

}
