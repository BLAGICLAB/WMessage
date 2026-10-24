pub(crate) const SKILL_NAME_CHARS_OK: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';

/// SKILL.md 完整元数据（Skill 运行模型 v1.0 字段集，全部带默认值）
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub risk_level: String, // low | medium | high
    pub mode: String,       // auto | interactive
    pub max_steps: usize,
    pub timeout_secs: u64,
    pub rollback: String, // none | auto
    pub enabled: bool,
    /// 是否支持暂停后断点续跑（false 时暂停即终止：确认完成当前动作后技能结束）
    pub resumable: bool,
    /// 会话档（frontmatter `scope: session`）：模型 create_skill 建的技能。
    /// 这类技能不进全局路由表（跨会话不劫持），只在创建会话内经
    /// intent_router::add_session_route 生效；重启后不再自动触发。
    pub session_scoped: bool,
    pub intents: Vec<String>,
    /// 多步 Skill 自报的对话轮数上限；
    /// None → 运行时 fallback bot_model_loop::DEFAULT_MAX_ROUNDS（50）。
    /// 解析时 clamp 到 1..=60（现有 Skill 未声明该字段 → None，不受影响）。
    pub max_rounds: Option<usize>,
    /// allowed-tools 前置声明（agentskills.io 开放标准字段）。
    /// 空 = 不限制（向后兼容）；非空 = 调度器步骤执行前校验 tool ∈ 声明集合
    /// （作者主动限权，与系统兜底「tool ∈ 注册表」正交两层）。
    pub allowed_tools: Vec<String>,
    /// 可选版本号（导入重名提示对比 + 设置页展示）
    pub version: Option<String>,
    /// 参数契约（frontmatter params 多行列表）
    pub params: Vec<SkillParam>,
}

/// 技能参数契约单条（frontmatter params 列表行）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillParam {
    pub key: String,
    pub desc: String,
    pub required: bool,
    pub default: Option<String>,
}

impl Default for SkillMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            risk_level: "medium".into(),
            mode: "interactive".into(),
            // 单技能步数硬上限的**默认值**（frontmatter `max_steps` 可覆盖，clamp 1..=20）。
            // 默认 20（= clamp 上限）：实测 gorden-ppt-skill 这类
            // 「读模板索引 → 探模板 JSON → 生成 PPT → 登记产物」的正常流程需 ~9 步，
            // 默认 8 会中途熔断（skill_failed），进而丢掉 link_file_to_task 的产物登记。
            // 防长流程仍由 timeout_secs（默认 180s，clamp 10..=600）与单轮 Function 上限兜底。
            max_steps: 20,
            timeout_secs: 180,
            rollback: "none".into(),
            enabled: true,
            resumable: false,
            session_scoped: false,
            intents: Vec::new(),
            max_rounds: None,
            allowed_tools: Vec::new(),
            version: None,
            params: Vec::new(),
        }
    }
}

/// params 列表行解析（纯函数，可测）。
/// 行格式：`- key: 说明文字（必填）` / `- key: 说明文字（默认 X）` / `- key: 说明文字`。
/// required 标记 = 说明含「必填」；默认值 = 说明含「默认 X」时提取 X；两者互斥。
fn parse_param_line(line: &str) -> Option<SkillParam> {
    let t = line.trim().strip_prefix("- ")?.trim();
    let (key, rest) = t.split_once(':')?;
    let key = key.trim().to_string();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let rest = rest.trim();
    let required = rest.contains("必填");
    let default = extract_default(rest);
    let desc = strip_annotations(rest);
    Some(SkillParam {
        key,
        desc,
        required,
        default,
    })
}

/// 提取「默认 X」标记里的 X（默认 X 外层括号全角/半角都认）；无默认 → None。
/// 只认「(默认 X)」形态——括号内以「默认」开头才算标记，
/// 说明文字里恰好含「默认」二字的普通括号注释不当默认值。
fn extract_default(rest: &str) -> Option<String> {
    let pos = rest.find("默认")?;
    let open = rest[..pos].rfind(['（', '('])?;
    let close = rest[pos..].find(['）', ')'])? + pos;
    // 全角/半角括号字节宽不同（3/1），按命中字符的实际宽度推进切片边界，
    // 硬编码全角宽度会把边界踩进多字节字符中间直接 panic
    let open_w = rest[open..].chars().next()?.len_utf8();
    let inner = rest[open + open_w..close].trim();
    let inner = inner.strip_prefix("默认")?.trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// 剥掉「（必填）」与「（默认 X）」标记后的纯说明文字。
/// 「(默认 X)」的识别口径与 extract_default 一致：括号内以「默认」开头才剥。
fn strip_annotations(rest: &str) -> String {
    let mut s = rest.to_string();
    for marker in ["（必填）", "(必填)"] {
        s = s.replace(marker, "");
    }
    if let Some(pos) = s.find("默认") {
        if let Some(open) = s[..pos].rfind(['（', '(']) {
            if let Some(rel) = s[pos..].find(['）', ')']) {
                // 括号字节宽全角/半角不同，切片边界按命中字符实际宽度推进（越界会 panic）
                let open_w = s[open..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                let close_pos = pos + rel;
                let close_w = s[close_pos..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
                // 只剥「(默认 X)」形态：括号内不以「默认」开头的是普通括号注释，不动
                if s[open + open_w..close_pos].trim().starts_with("默认") {
                    s = format!("{}{}", &s[..open], &s[close_pos + close_w..]);
                }
            }
        }
    }
    s.trim()
        .trim_end_matches('：')
        .trim_end_matches(':')
        .to_string()
}

/// 解析完整元数据（调度器用）：缺失字段走默认值；非法值回退默认
pub fn parse_meta(text: &str, dir_name: &str) -> SkillMeta {
    let mut m = SkillMeta::default();
    // 剥离 UTF-8 BOM——带 BOM 的文件 frontmatter 会整体静默丢失
    // （name 退目录名、mode 退默认、intents 丢失 → 路由失效），无任何告警
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let body = text.strip_prefix("---").unwrap_or(text);
    let Some(end) = body.find("\n---") else {
        m.name = dir_name.to_string();
        return m;
    };
    let fm = &body[..end];
    let fm_lines: Vec<&str> = fm.lines().collect();
    let mut li = 0;
    // mode 是否显式声明（风险推导需要区分「没写」与「显式写默认值」）
    let mut mode_explicit = false;
    while li < fm_lines.len() {
        let line = fm_lines[li].trim();
        li += 1;
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        // 单引号 YAML scalar 与双引号同权剥离（原仅剥双引号：'foo-bar' 带引号进值）
        let v = v.trim().trim_matches(['"', '\'']);
        match k.trim() {
            "name" => m.name = v.to_string(),
            "description" => m.description = v.to_string(),
            "risk_level" => {
                let v2 = v.to_ascii_lowercase();
                m.risk_level = if matches!(v2.as_str(), "low" | "medium" | "high") {
                    v2
                } else {
                    "medium".into()
                }
            }
            "mode" => {
                let v2 = v.to_ascii_lowercase();
                m.mode = if matches!(v2.as_str(), "auto" | "interactive") {
                    mode_explicit = true;
                    v2
                } else {
                    // 非法 mode 按未声明处理：下方风险推导接管，
                    // low 未显式声明 → auto 提升不被非法值吞掉
                    "interactive".into()
                }
            }
            "max_steps" => {
                if let Ok(n) = v.parse::<usize>() {
                    m.max_steps = n.clamp(1, 20);
                }
            }
            "timeout_secs" => {
                if let Ok(n) = v.parse::<u64>() {
                    m.timeout_secs = n.clamp(10, 600);
                }
            }
            "max_rounds" => {
                if let Ok(n) = v.parse::<usize>() {
                    m.max_rounds = Some(n.clamp(1, 60));
                }
            }
            "rollback" => {
                let v2 = v.to_ascii_lowercase();
                m.rollback = if matches!(v2.as_str(), "none" | "auto") {
                    v2
                } else {
                    "none".into()
                }
            }
            "enabled" => {
                m.enabled = !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no");
            }
            // 会话档标记：模型 create_skill 建的技能被强制写为 scope: session——
            // 全局路由扫描（intent_rules_from_dirs）跳过这类技能，路由只经
            // add_session_route 进会话档。用户想提升为全局：改掉这行即可。
            "scope" => {
                m.session_scoped = v.trim().eq_ignore_ascii_case("session");
            }
            "resumable" => {
                m.resumable =
                    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes");
            }
            "intents" => {
                let inner = v.trim();
                if inner.is_empty() {
                    // 多行 YAML 列表：intents: 后随 `- ` 行逐条收取。
                    // 模式含逗号（如量词 {0,15}）必须用此形式——单行逗号分隔会切碎量词。
                    while li < fm_lines.len() {
                        let t = fm_lines[li].trim();
                        let Some(item) = t.strip_prefix("- ") else {
                            break;
                        };
                        let item = item.trim().trim_matches('"').trim_matches('\'');
                        if !item.is_empty() {
                            m.intents.push(item.to_string());
                        }
                        li += 1;
                    }
                } else {
                    // 单行：["a","b"] 或逗号分隔 "a,b"（模式内不得含逗号，含逗号请用多行列表）
                    m.intents = inner
                        .trim_matches(['[', ']'])
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
            // allowed-tools（agentskills.io 开放标准字段）——单行逗号分隔或
            // 多行 `- ` 列表；工具名与注册表命名同规范（ASCII 字母数字下划线）
            "allowed-tools" | "allowed_tools" => {
                let inner = v.trim();
                if inner.is_empty() {
                    while li < fm_lines.len() {
                        let t = fm_lines[li].trim();
                        let Some(item) = t.strip_prefix("- ") else {
                            break;
                        };
                        let item = item.trim().trim_matches('"').trim_matches('\'');
                        if !item.is_empty() {
                            m.allowed_tools.push(item.to_string());
                        }
                        li += 1;
                    }
                } else {
                    m.allowed_tools = inner
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
            // 可选版本号
            "version" => {
                if !v.is_empty() {
                    m.version = Some(v.to_string());
                }
            }
            // params 多行列表（每行 `- key: 说明（必填）/（默认 X）`）
            "params" => {
                let inner = v.trim();
                if !inner.is_empty() {
                    // 单行不支持（说明文字含冒号/括号），声明了单行当无参数处理
                    continue;
                }
                while li < fm_lines.len() {
                    let t = fm_lines[li];
                    if t.trim().starts_with("- ") {
                        if let Some(p) = parse_param_line(t) {
                            m.params.push(p);
                        }
                        li += 1;
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    if m.name.is_empty() || !m.name.chars().all(SKILL_NAME_CHARS_OK) {
        m.name = dir_name.to_string();
    }
    // 风险推导模式（显式 mode 优先）：high → 强制 interactive（安全兜底）；
    // low + 未显式声明 mode → auto（SKILL_DSL.md 约定「low 强制 auto」）
    if m.risk_level == "high" {
        m.mode = "interactive".into();
    } else if m.risk_level == "low" && !mode_explicit {
        m.mode = "auto".into();
    }
    m
}

/// frontmatter 内容区间（与 parse_meta 的边界判定**同口径**）：
/// 返回 `(起始字节, 结束字节)`——区间内是首尾 `---` 之间的内容。
///
/// **坐标基准 = 传入文本本身**（调用方须传入 BOM 已剥离的文本）。
/// 与 parse_meta 走完全同一条规则（`strip_prefix("---")` + `find("\n---")`）：
/// 写入侧与解析侧若用两套边界判定，就会出现「拨杆点了没反应」
/// （写进去的 `enabled` 落在解析器看不见的地方）。
///
/// 只在文本以 `---` 开头时定位——parse_meta 在这种情形下正是这么切的。
/// 文本不以 `---` 开头时 parse_meta 会退而全文搜 `\n---`（把正文当 frontmatter
/// 的病态文件），此时本函数返回 None → 走「新建 frontmatter」分支，
/// 改写后的文件解析器一定看得见 `enabled`。
fn frontmatter_span(text: &str) -> Option<(usize, usize)> {
    let body = text.strip_prefix("---")?;
    let end = body.find("\n---")?;
    Some((3, 3 + end))
}

/// 改写 frontmatter 的 `enabled` 键（纯函数，可单测）。
///
/// 语义：
/// - 有 frontmatter 且已声明 `enabled` → 原地改值（整行替换，保留缩进与行尾）
/// - 有 frontmatter 但未声明 → 在 frontmatter 内容区**最前面**插入 `enabled: true|false`
/// - 完全没有 frontmatter → 在文件头补一个只含 `enabled` 的 frontmatter
///   （`name` 仍回退目录名、`description` 仍为空 → 与改写前的解析结果一致）
///
/// 整行替换而非只改值：解析器**不剥行尾注释**
/// （`enabled: false  # 关闭原因` 会被读成 enabled=true——值不是 "false"）。
/// 留注释 = 开关读不准 → 整行归一反而把值定死。
///
/// 收尾自检：改写后的文本必须能被 `parse_meta` 读回目标值，读不回就返回
/// `None`——宁可不写，也不写一份「开关点了没反应」的技能。
pub fn set_frontmatter_enabled(text: &str, enabled: bool) -> Option<String> {
    // BOM 先剥离：parse_meta 也是先剥 BOM 再认 frontmatter，坐标须同基准。
    // 剥下的 BOM 在最后原样接回（带 BOM 的文件 frontmatter 会整体静默丢失，
    // 但那是既有解析行为，写入侧不该顺手改变文件编码）。
    //
    // strip_prefix 返回的是**剩余部分**而非匹配的前缀 → BOM 本身要单独取。
    let had_bom = text.starts_with('\u{feff}');
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let key_line = format!("enabled: {}", if enabled { "true" } else { "false" });

    let rewritten = match frontmatter_span(body) {
        Some((start, end)) => {
            let fm = &body[start..end];
            let new_fm = match last_enabled_line(fm) {
                // 改值：保留原行缩进与行尾，其余字节原样
                Some(range) => {
                    let orig = &fm[range.clone()];
                    let lead = orig.len() - orig.trim_start().len();
                    format!(
                        "{}{}{}{}",
                        &fm[..range.start],
                        &orig[..lead],
                        key_line,
                        &fm[range.end..]
                    )
                }
                // 未声明 → 插在 frontmatter 内容区最前面（不落在别的键的续行里）
                None => insert_key_line(fm, &key_line),
            };
            format!("{}{}{}", &body[..start], new_fm, &body[end..])
        }
        // 无 frontmatter → 补一个只含 enabled 的（解析结果与改写前一致）
        None => format!("---\n{}\n---\n{}", key_line, body),
    };

    // 写完必须能被解析器读回目标值
    if parse_meta(&rewritten, "").enabled != enabled {
        return None;
    }
    Some(if had_bom {
        format!("\u{feff}{}", rewritten)
    } else {
        rewritten
    })
}

/// frontmatter 里**最后一个** `enabled` 键所在行的字节区间（不含行尾换行）。
///
/// 与 parse_meta **逐行同口径**，两处规则必须一致：
/// - 键判定：整行 trim 后按首个 ':' 切、键名再 trim（parse_meta 同款）
/// - 多行块跳过：`intents` / `params` / `allowed-tools` 的空值形式会吞掉后续
///   `- ` 开头的行，那些行**不是顶层键**（把 `allowed-tools` 里的工具名
///   当成 enabled 键改掉 = 改坏了技能）
/// - 取最后一个：parse_meta 逐行赋值，同名键**后者覆盖前者**——改最后一个
///   才与解析器实际生效的那一个对齐
fn scan_last_key_line(fm: &str, target: &str) -> Option<std::ops::Range<usize>> {
    let mut found: Option<std::ops::Range<usize>> = None;
    let mut offset = 0usize;
    // 上一行是否开启了「多行块」——块内 `- ` 开头的行不是顶层键
    let mut in_block = false;
    for chunk in fm.split_inclusive('\n') {
        // 行内容不含行尾的 \r / \n（CRLF 也算）
        let content_len = chunk.trim_end_matches(['\n', '\r']).len();
        let content = &chunk[..content_len];
        let trimmed = content.trim();
        if in_block {
            if trimmed.starts_with("- ") {
                offset += chunk.len();
                continue;
            }
            in_block = false;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            // 值去引号（parse_meta 在进 match 前就剥了）后再判空
            let val = v.trim().trim_matches(['"', '\'']);
            // 这三个键的空值形式会吞掉后续 `- ` 行（parse_meta 同款）
            if val.is_empty()
                && matches!(
                    key,
                    "intents" | "params" | "allowed-tools" | "allowed_tools"
                )
            {
                in_block = true;
            }
            if key == target {
                found = Some(offset..offset + content_len);
            }
        }
        offset += chunk.len();
    }
    found
}

/// frontmatter 里**最后一个** `enabled` 键所在行（[`last_key_line`] 的固定键名薄壳）
fn last_enabled_line(fm: &str) -> Option<std::ops::Range<usize>> {
    scan_last_key_line(fm, "enabled")
}

/// frontmatter 里**最后一个**指定键所在行的字节区间（不含行尾换行）。
/// 与 parse_meta 逐行同口径（键判定 / 多行块跳过 / 后者覆盖前者），
/// 详见 [`last_enabled_line`] 的历史注释——泛化自那个函数，规则没变。
fn last_key_line(fm: &str, target: &str) -> Option<std::ops::Range<usize>> {
    scan_last_key_line(fm, target)
}

/// 在 frontmatter 里插入一个键行（纯函数，可单测）。
///
/// 插入点 = frontmatter 内容区（`---` 之后、闭合 `---` 之前）的**最前面**。
/// 插在末尾会落进别的键的续行里（`intents:` / `params:` / `allowed-tools:`
/// 的空值形式吞掉后续 `- ` 行），插在中间会劈开某个多行块——都改坏技能。
///
/// frontmatter 内容区通常以「`---` 后的换行」开头，插在它之后 = 插在首个键之前；
/// `---` 与首键粘连（无前导换行）的病态文件则补一个换行再插。
fn insert_key_line(fm: &str, line: &str) -> String {
    let rest = fm.strip_prefix('\n').unwrap_or(fm);
    if rest.is_empty() {
        format!("\n{}", line)
    } else {
        format!("\n{}\n{}", line, rest)
    }
}

/// 把技能正文 frontmatter 的 `scope` 键强制写成 `session`（已声明则整行替换，
/// 未声明则插在内容区最前面）。create_skill 落盘前调用——模型 content 里就算
/// 写了别的 scope 值也会被覆盖：会话档是工具来源技能的结构属性（跨会话不
/// 劫持、重启即失效），不是模型可自选的项。
/// 无 frontmatter → None（create_skill 的准入核已保证 frontmatter 在场）。
pub fn force_frontmatter_session_scope(text: &str) -> Option<String> {
    let had_bom = text.starts_with('\u{feff}');
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let key_line = "scope: session";

    let rewritten = match frontmatter_span(body) {
        Some((start, end)) => {
            let fm = &body[start..end];
            let new_fm = match last_key_line(fm, "scope") {
                // 改值：保留原行缩进与行尾，其余字节原样
                Some(range) => {
                    let orig = &fm[range.clone()];
                    let lead = orig.len() - orig.trim_start().len();
                    format!(
                        "{}{}{}{}",
                        &fm[..range.start],
                        &orig[..lead],
                        key_line,
                        &fm[range.end..]
                    )
                }
                None => insert_key_line(fm, key_line),
            };
            format!("{}{}{}", &body[..start], new_fm, &body[end..])
        }
        None => return None,
    };

    // 写完必须能被解析器读回目标值（与 set_frontmatter_enabled 同口径）
    if !parse_meta(&rewritten, "").session_scoped {
        return None;
    }
    Some(if had_bom {
        format!("\u{feff}{}", rewritten)
    } else {
        rewritten
    })
}

/// frontmatter 里**显式声明**的 `enabled` 值（未声明 → None）。
///
/// 与 `parse_meta` 同口径取**最后一个** `enabled` 键（同名键后者覆盖前者）。
/// 「未声明」与「声明为 true」必须能区分——内置技能物化时要靠这个判断
/// 「用户是不是手动关过它」（没声明 ≠ 声明 true）。
pub fn frontmatter_enabled_declared(text: &str) -> Option<bool> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (start, end) = frontmatter_span(body)?;
    let fm = &body[start..end];
    let range = last_enabled_line(fm)?;
    let line = &fm[range];
    let (_, v) = line.split_once(':')?;
    let val = v.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    Some(!matches!(val.as_str(), "false" | "0" | "no"))
}

// DSL 解析 + 调度器

/// Skill 步骤（DSL 解析后的结构）
///
/// Markdown DSL 格式（仅 `mode: "auto"` 的 Skill 走调度器）：
///   ## Step N: 标题
///   tool_name({"arg": "value"})
///   ## Rollback
///   undo_tool({"arg": "value"})
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillStep {
    /// 1-based 步骤序号（用于错误信息 / 回滚顺序）
    pub index: usize,
    pub title: String,
    /// 工具名（必须存在于 TOOLS 白名单）
    pub tool_name: String,
    /// JSON 参数串（直接透传给 `bot::execute_tool`）
    pub args_json: String,
}

/// 剥离 YAML frontmatter（`---` ... `---`），返回正文部分。
pub(crate) fn strip_frontmatter(text: &str) -> &str {
    // 先剥 UTF-8 BOM，否则 frontmatter 探测整体失效
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.starts_with("---") {
        return text;
    }
    let body = &text[3..];
    match body.find("\n---") {
        Some(end) => body[end + 4..].trim_start_matches('\n'),
        None => text,
    }
}

/// 回滚段标题判定（统一三套说法）：
/// `## Rollback` / `## Rollback ...` / `## 回滚` / `## 回滚（Rollback）` 都认。
/// parse_skill_steps 与 runtime::rollback_section 共用同一判定，不再各认一半。
pub(crate) fn is_rollback_heading(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("## ") else {
        return false;
    };
    rest == "Rollback"
        || rest.starts_with("Rollback ")
        || rest == "回滚"
        || rest.starts_with("回滚 ")
        || rest.starts_with("回滚（")
        || rest.starts_with("回滚(")
}

/// 正文里有没有回滚段。
///
/// 与 `parse_skill_steps` 共用 `is_rollback_heading` 同一判定——两处各认一半
/// 会漂（判据不一致 = 「声明了 auto 却说没有回滚段」这类假象）。
pub(crate) fn body_has_rollback_section(body: &str) -> bool {
    body.lines().any(is_rollback_heading)
}

/// `rollback: auto` 但正文没有 `## Rollback` 段 → 降为 `none`。
///
/// SKILL-DSL 承诺过这个降级：声明了 auto 却没有回滚段可跑 = 没有回滚。
/// 不降级的话，「回滚 auto」会被读成「会回滚」，而实际上一段都没有。
///
/// 判据单源在这里：frontmatter 单独解析时看不到正文，所以降级必须由
/// 「拿到 meta 与正文」的那一层调用。
pub(crate) fn normalize_rollback_meta(mut m: SkillMeta, body: &str) -> SkillMeta {
    if m.rollback == "auto" && !body_has_rollback_section(body) {
        m.rollback = "none".into();
    }
    m
}

/// 解析 `## Step N: 标题` 或 `## Step N 标题` → `(序号, 标题)`
fn parse_step_heading(line: &str) -> Option<(usize, String)> {
    let rest = line.trim().strip_prefix("## Step ")?;
    let (num_str, title) = if let Some(colon) = rest.find(':') {
        (&rest[..colon], rest[colon + 1..].trim())
    } else if let Some(space) = rest.find(' ') {
        (&rest[..space], rest[space + 1..].trim())
    } else {
        (rest, "")
    };
    let num: usize = num_str.trim().parse().ok()?;
    Some((num, title.to_string()))
}

/// 解析一行工具调用：`tool_name({"arg": "value"})` → `(name, args)`
/// 约束：单行、结尾 `)`、name 为 ASCII 字母数字下划线；args 可含任意字符（含嵌套括号）
fn parse_tool_call(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if !line.ends_with(')') {
        return None;
    }
    let open = line.find('(')?;
    let name = line[..open].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let args = &line[open + 1..line.len() - 1];
    Some((name.to_string(), args.to_string()))
}

/// 解析 Skill body 为 (主步骤, 回滚步骤)。
/// - 空 body / 无 step → `(Vec::new(), Vec::new())`
/// - 解析失败 → Err
/// 解析规则：
/// - 回滚段标题走 is_rollback_heading 统一判定（`## Rollback` / `## 回滚` 都认）
/// - 回滚段每行工具调用是一个独立回滚步骤
/// - Step 内含多行工具调用 → Err
/// - 步骤序号重复 → Err（`${stepN.*}` 替换取首个匹配，重号会静默错位）
pub fn parse_skill_steps(body: &str) -> Result<(Vec<SkillStep>, Vec<SkillStep>), String> {
    let body = strip_frontmatter(body);
    let mut steps: Vec<SkillStep> = Vec::new();
    let mut rollback: Vec<SkillStep> = Vec::new();
    let mut current: Option<SkillStep> = None;
    let mut mode: &str = "step"; // "step" | "rollback"

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((idx, title)) = parse_step_heading(line) {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            current = Some(SkillStep {
                index: idx,
                title,
                tool_name: String::new(),
                args_json: String::new(),
            });
            mode = "step";
        } else if is_rollback_heading(line) {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            mode = "rollback";
        } else if line.starts_with('#') {
            continue;
        } else if let Some((name, args)) = parse_tool_call(line) {
            if mode == "step" {
                match current.as_mut() {
                    Some(s) if s.tool_name.is_empty() => {
                        s.tool_name = name;
                        s.args_json = args;
                    }
                    Some(s) => {
                        return Err(format!(
                            "Step {}（{}）含多行工具调用，每步只支持一行——请拆成多个 Step",
                            s.index, s.title
                        ));
                    }
                    None => {} // 步骤外的孤立工具行忽略（保持现状）
                }
            } else {
                // 回滚段：每行工具调用是一个独立回滚步骤
                rollback.push(SkillStep {
                    index: rollback.len() + 1,
                    title: "rollback".into(),
                    tool_name: name,
                    args_json: args,
                });
            }
        }
    }
    if let Some(s) = current {
        steps.push(s);
    }
    let mut seen = std::collections::HashSet::new();
    for s in &steps {
        if !seen.insert(s.index) {
            return Err(format!(
                "步骤序号 {} 重复——${{stepN.result}} 替换会静默错位，请修正编号为连续不重复",
                s.index
            ));
        }
    }
    // 编号必须从 1 连续递增：空洞会让 ${stepN.*} 引用静默落空（Step 1→3 时 step2 引用悬空）
    for (i, s) in steps.iter().enumerate() {
        if s.index != i + 1 {
            return Err(format!(
                "步骤编号必须从 1 连续递增（第 {} 条实际编号 {}）——${{stepN.*}} 引用会静默落空",
                i + 1,
                s.index
            ));
        }
    }
    Ok((steps, rollback))
}

/// 技能步骤/回滚引用的工具名 vs 内置注册表 → 未知工具清单（去重保序）。
/// MCP 挂载的工具不在内置注册表内——调用方对未知项只警告不阻断（注明可能为 MCP 工具）。
pub fn unknown_tool_names(
    steps: &[SkillStep],
    rollback: &[SkillStep],
    known: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in steps.iter().chain(rollback.iter()) {
        if s.tool_name.is_empty() || known.contains(&s.tool_name) {
            continue;
        }
        if !out.contains(&s.tool_name) {
            out.push(s.tool_name.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_skills::{extract_task_id, substitute_vars, CompletedStep};

    // ── 元数据解析 ──

    #[test]
    fn scope_session_key_parses_into_meta() {
        let m = parse_meta(
            "---\nname: a\ndescription: d\nscope: session\n---\n正文",
            "",
        );
        assert!(m.session_scoped, "scope: session 必须解析成会话档");
        let m2 = parse_meta("---\nname: a\ndescription: d\n---\n正文", "");
        assert!(!m2.session_scoped, "未声明 scope = 全局档（默认）");
        let m3 = parse_meta("---\nname: a\ndescription: d\nscope: global\n---\n正文", "");
        assert!(!m3.session_scoped, "scope: global 显式全局，不是会话档");
    }

    #[test]
    fn force_session_scope_inserts_when_absent() {
        let src = "---\nname: a\ndescription: d\nintents:\n  - 润色文档\n---\n正文";
        let out = force_frontmatter_session_scope(src).expect("应注入成功");
        assert!(
            out.contains("scope: session"),
            "注入后的 frontmatter 必须含 scope: session"
        );
        let m = parse_meta(&out, "");
        assert!(m.session_scoped, "注入后必须能被解析器读回会话档");
        assert!(
            m.intents.contains(&"润色文档".to_string()),
            "注入不得劈坏多行 intents 块：{out}"
        );
    }

    #[test]
    fn force_session_scope_overrides_any_existing_value() {
        let src = "---\nname: a\ndescription: d\nscope: global\n---\n正文";
        let out = force_frontmatter_session_scope(src).expect("应改写成功");
        assert!(
            !out.contains("scope: global"),
            "模型写的 scope 值必须被覆盖（会话档不是模型可自选项）"
        );
        assert!(parse_meta(&out, "").session_scoped);
    }

    #[test]
    fn force_session_scope_rejects_frontmatterless_text() {
        assert!(
            force_frontmatter_session_scope("没有 frontmatter 的正文").is_none(),
            "无 frontmatter 无法标记，宁可失败也不静默放行"
        );
    }

    #[test]
    fn meta_parses_full_fields() {
        let text = "---\nname: daily-archive\ndescription: 汇总归档\nrisk_level: medium\nmode: interactive\nmax_steps: 6\ntimeout_secs: 120\nrollback: auto\nenabled: true\nintents: [\"日报\",\"归档\"]\n---\n# 步骤\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(m.name, "daily-archive");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.mode, "interactive");
        assert_eq!(m.max_steps, 6);
        assert_eq!(m.timeout_secs, 120);
        assert_eq!(m.rollback, "auto");
        assert!(m.enabled);
        assert_eq!(m.intents, vec!["日报", "归档"]);
    }

    #[test]
    fn meta_intents_multiline_list_preserves_quantifier_commas() {
        // intents 多行 YAML 列表形式。
        // 关键：模式内的量词逗号（{0,15}）不得被切碎——单行逗号分隔形式做不到，所以路由模式用多行列表
        let text = "---\nname: minimax-docx\ndescription: d\ntriggers:\n  - Word\n  - 文档\nintents:\n  - '(?i)(润色|修订).{0,15}(word|文档)'\n  - '(?is)(润色|修订)[\\s\\S]*\\.docx?'\n---\n# body\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(
            m.intents,
            vec![
                r"(?i)(润色|修订).{0,15}(word|文档)",
                r"(?is)(润色|修订)[\s\S]*\.docx?",
            ]
        );
        // triggers 列表行不得混入 intents（intents 之前的 `- ` 行不属于本字段）
        assert!(!m.intents.iter().any(|i| i == "Word"));
    }

    #[test]
    fn meta_intents_empty_without_declaration() {
        // 未声明 intents → 空（动态路由下该技能不产生路由条目）
        let m = parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert!(m.intents.is_empty());
    }

    #[test]
    fn meta_defaults_and_clamps() {
        let m = parse_meta("没有 frontmatter", "dir-x");
        assert_eq!(m.name, "dir-x");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.max_steps, 20); // 默认 = clamp 上限
        assert_eq!(m.timeout_secs, 180);
        assert!(!m.rollback.eq("auto"));

        // 非法值回退 + 越界 clamp
        let t = "---\nrisk_level: dangerous\nmode: auto\nmax_steps: 999\ntimeout_secs: 1\n---\n";
        let m2 = parse_meta(t, "d");
        assert_eq!(m2.risk_level, "medium"); // 非法回退
        assert_eq!(m2.mode, "auto"); // 合法保留
        assert_eq!(m2.max_steps, 20); // clamp 上限
        assert_eq!(m2.timeout_secs, 10); // clamp 下限
    }

    #[test]
    fn meta_high_risk_forces_interactive() {
        let t = "---\nrisk_level: high\nmode: auto\n---\n";
        let m = parse_meta(t, "d");
        assert_eq!(m.mode, "interactive"); // 安全兜底：high 强制人机协同
    }

    #[test]
    fn meta_invalid_mode_does_not_block_low_auto_promotion() {
        // 非法 mode 按未声明处理：low 未显式声明 → auto 提升不被吞
        let m = parse_meta("---\nmode: bg\nrisk_level: low\n---\n", "d");
        assert_eq!(m.mode, "auto");
        // 合法显式声明保留（不受提升逻辑改写）
        let m2 = parse_meta("---\nmode: interactive\nrisk_level: low\n---\n", "d");
        assert_eq!(m2.mode, "interactive");
        let m3 = parse_meta("---\nmode: auto\nrisk_level: low\n---\n", "d");
        assert_eq!(m3.mode, "auto");
    }

    #[test]
    fn meta_disabled_flag() {
        let t = "---\nenabled: false\n---\n";
        let m = parse_meta(t, "d");
        assert!(!m.enabled);
    }

    // ── enabled 键改写（设置页拨杆的落盘内核） ──

    /// 改写后必须能被解析器读回目标值——这是「拨杆点了没反应」的唯一防线。
    #[test]
    fn set_enabled_flips_declared_value_and_keeps_other_keys() {
        let src = "---\nname: demo\ndescription: 演示\nenabled: true\nmode: auto\n---\n正文\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        let m = parse_meta(&out, "demo");
        assert!(!m.enabled, "关闭后解析器必须读到 false");
        assert_eq!(m.name, "demo");
        assert_eq!(m.description, "演示");
        assert_eq!(m.mode, "auto", "其他键不得被改写波及");
        assert!(out.contains("正文"), "正文必须原样保留");
    }

    #[test]
    fn set_enabled_inserts_key_when_absent() {
        let src = "---\nname: demo\ndescription: 演示\n---\n正文\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(!parse_meta(&out, "demo").enabled);
        assert!(out.contains("enabled: false"), "应插入 enabled 键");
        assert_eq!(parse_meta(&out, "demo").name, "demo");
    }

    /// 未声明 enabled 的技能默认是启用的（parse_meta 默认 true），
    /// 插入不能把「默认启用」误改成「显式禁用」。
    #[test]
    fn set_enabled_inserts_true_when_turning_on_undeclared_skill() {
        let src = "---\nname: demo\n---\n";
        let out = set_frontmatter_enabled(src, true).expect("应可改写");
        assert!(parse_meta(&out, "demo").enabled);
        assert!(out.contains("enabled: true"));
    }

    /// 完全没有 frontmatter 的文件：补一个只含 enabled 的 frontmatter，
    /// 其余解析结果（name 回退目录名 / description 空）保持不变。
    #[test]
    fn set_enabled_creates_frontmatter_when_absent() {
        let src = "# 标题\n正文\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        let m = parse_meta(&out, "demo-dir");
        assert!(!m.enabled);
        assert_eq!(m.name, "demo-dir", "name 仍回退目录名");
        assert!(m.description.is_empty(), "description 仍为空");
        assert!(out.contains("# 标题"), "正文必须原样保留");
    }

    /// BOM 必须保留：带 BOM 的文件 frontmatter 会整体静默丢失（既有解析行为），
    /// 写入侧不该顺手把文件的编码改了。
    #[test]
    fn set_enabled_preserves_bom() {
        let src = "\u{feff}---\nname: demo\nenabled: true\n---\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(out.starts_with('\u{feff}'), "BOM 必须保留");
        assert!(
            !parse_meta(&out, "demo").enabled,
            "BOM 存在时解析器仍须读到 false"
        );
    }

    /// CRLF 行尾：改写不得把 \r\n 变成 \n（Windows 上 SKILL.md 是 CRLF 的）。
    #[test]
    fn set_enabled_preserves_crlf() {
        let src = "---\r\nname: demo\r\nenabled: true\r\n---\r\n正文\r\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(out.contains("\r\n"), "CRLF 行尾必须保留");
        assert!(!out.contains("\n\n"), "不得混入裸 LF");
        assert!(!parse_meta(&out, "demo").enabled);
    }

    /// 解析器不剥行尾注释（`enabled: false  # 关闭原因` 的值不是 "false"
    /// → 会被读成启用）。整行替换把值定死，不留注释。
    #[test]
    fn set_enabled_normalizes_line_with_trailing_comment() {
        let src = "---\nname: demo\nenabled: true  # 默认开\n---\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(
            !parse_meta(&out, "demo").enabled,
            "带注释的行必须读到 false"
        );
        assert!(!out.contains("# 默认开"), "整行替换不保留行尾注释");
    }

    /// 多行块（intents / params / allowed-tools）的续行不是顶层键——
    /// 把块内的 `- enabled: xxx` 当成 enabled 键改掉 = 改坏了技能。
    #[test]
    fn set_enabled_does_not_touch_multiline_block_items() {
        let src = "---\nname: demo\nallowed-tools:\n  - enabled\n  - read_file\n---\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(out.contains("- enabled"), "块内条目不得被改写");
        assert!(out.contains("- read_file"), "块内条目不得被改写");
        assert!(!parse_meta(&out, "demo").enabled, "仍须写入顶层 enabled");
    }

    /// 同名键后者覆盖前者（parse_meta 逐行赋值）→ 改最后一个才与生效值对齐。
    /// 用缩进区分两个同名键：改错了第一个，结果是「两个 false」；
    /// 改对了最后一个，结果是「缩进的 true + 平的 false」。
    #[test]
    fn set_enabled_targets_the_last_declared_key() {
        let src = "---\nname: demo\n  enabled: true\nenabled: false\n---\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(
            out.contains("  enabled: true\nenabled: false"),
            "只应改最后一个 enabled 键（前者原样保留），got:\n{out}"
        );
        assert!(!parse_meta(&out, "demo").enabled, "最后一个键决定生效值");
    }

    /// 缩进保留：frontmatter 里的缩进风格不该被改写抹掉。
    #[test]
    fn set_enabled_preserves_indentation() {
        let src = "---\nname: demo\n  enabled: true\n---\n";
        let out = set_frontmatter_enabled(src, false).expect("应可改写");
        assert!(out.contains("  enabled: false"), "缩进必须保留");
        assert!(!parse_meta(&out, "demo").enabled);
    }

    /// frontmatter_enabled_declared：区分「没声明」与「声明为 true」
    /// （内置技能物化时据此判断用户是不是手动关过它）。
    #[test]
    fn frontmatter_enabled_declared_distinguishes_absent_from_true() {
        assert_eq!(frontmatter_enabled_declared("---\nname: x\n---\n"), None);
        assert_eq!(
            frontmatter_enabled_declared("---\nname: x\nenabled: true\n---\n"),
            Some(true)
        );
        assert_eq!(
            frontmatter_enabled_declared("---\nname: x\nenabled: false\n---\n"),
            Some(false)
        );
        assert_eq!(
            frontmatter_enabled_declared("---\nname: x\nenabled: 0\n---\n"),
            Some(false),
            "0 / no / false 三种写法都算关闭（与 parse_meta 同口径）"
        );
    }

    #[test]
    fn meta_parses_resumable() {
        let t = "---\nresumable: true\n---\n";
        let m = parse_meta(t, "d");
        assert!(m.resumable);
        let t2 = "---\nresumable: false\n---\n";
        assert!(!parse_meta(t2, "d").resumable);
        assert!(!parse_meta("无 frontmatter", "d").resumable); // 默认 false
    }

    // ── ：allowed-tools / version / params 解析 ──

    #[test]
    fn meta_allowed_tools_single_line_and_multiline() {
        let single = "---\nallowed-tools: read_text_file, grep_files\n---\n";
        let m = parse_meta(single, "d");
        assert_eq!(m.allowed_tools, vec!["read_text_file", "grep_files"]);
        let multi = "---\nallowed-tools:\n  - read_text_file\n  - edit_file\n---\n";
        let m2 = parse_meta(multi, "d");
        assert_eq!(m2.allowed_tools, vec!["read_text_file", "edit_file"]);
        // 未声明 = 空 = 不限制（向后兼容）
        assert!(parse_meta("---\nname: x\n---\n", "x")
            .allowed_tools
            .is_empty());
    }

    #[test]
    fn meta_version_optional() {
        let m = parse_meta("---\nversion: 2.1.0\n---\n", "d");
        assert_eq!(m.version.as_deref(), Some("2.1.0"));
        assert!(parse_meta("---\nname: x\n---\n", "x").version.is_none());
    }

    #[test]
    fn meta_params_multiline_with_required_and_default() {
        let text = "---\nname: code-fix\nparams:\n  - file: 目标文件绝对路径（必填）\n  - lang: 语言（默认 rust）\n  - note: 备注\n---\nbody\n";
        let m = parse_meta(text, "d");
        assert_eq!(m.params.len(), 3);
        assert_eq!(m.params[0].key, "file");
        assert!(m.params[0].required, "file 应为必填");
        assert_eq!(m.params[0].desc, "目标文件绝对路径");
        assert_eq!(m.params[1].key, "lang");
        assert!(!m.params[1].required);
        assert_eq!(m.params[1].default.as_deref(), Some("rust"));
        assert_eq!(m.params[1].desc, "语言");
        assert_eq!(m.params[2].key, "note");
        assert!(!m.params[2].required);
        assert!(m.params[2].default.is_none());
    }

    #[test]
    fn meta_params_invalid_key_line_skipped() {
        // key 含非法字符的行跳过（不中断其余参数解析）
        let text = "---\nparams:\n  - bad-key!: x\n  - ok_field: 正常（必填）\n---\n";
        let m = parse_meta(text, "d");
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].key, "ok_field");
    }

    #[test]
    fn meta_params_half_width_parens_no_panic() {
        // 半角括号字节宽与全角不同（1 vs 3）：切片边界必须按命中字符实际宽度推进，
        // 之前硬编码全角宽度会把边界踩进多字节字符中间直接 panic
        let text = "---\nparams:\n  - lang: 语言 (默认 rust)\n---\n";
        let m = parse_meta(text, "d");
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].default.as_deref(), Some("rust"));
        assert_eq!(m.params[0].desc, "语言");
    }

    #[test]
    fn meta_params_plain_paren_with_moji_not_default() {
        // 说明文字里恰好含「默认」二字的普通括号注释不是默认值标记，不提取也不剥
        let text = "---\nparams:\n  - mode: 选择模式（本 skill 默认行为）\n---\n";
        let m = parse_meta(text, "d");
        assert_eq!(m.params.len(), 1);
        assert!(m.params[0].default.is_none());
        assert_eq!(m.params[0].desc, "选择模式（本 skill 默认行为）");
    }

    #[test]
    fn frontmatter_parses_name_and_description() {
        let text = "---\nname: ppt-pro\ndescription: \"PPT 制作规范\"\n---\n# 正文\n步骤一\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(m.name, "ppt-pro");
        assert_eq!(m.description, "PPT 制作规范");
    }

    #[test]
    fn frontmatter_missing_falls_back_to_dir_name() {
        let m = parse_meta("没有 frontmatter", "dir-x");
        assert_eq!(m.name, "dir-x");
        assert_eq!(m.description, "");
    }

    #[test]
    fn frontmatter_invalid_name_falls_back() {
        let text = "---\nname: \"../evil\"\ndescription: x\n---\n";
        let m = parse_meta(text, "dir-x");
        assert_eq!(m.name, "dir-x"); // 非法名（含路径字符）回退目录名
    }

    #[test]
    fn read_skill_rejects_path_traversal() {
        // 直接验证 name 校验函数逻辑（无需 AppHandle）
        let ok = |s: &str| !s.is_empty() && s.chars().all(SKILL_NAME_CHARS_OK);
        assert!(ok("ppt-pro"));
        assert!(ok("minimax_docx"));
        assert!(!ok("../evil"));
        assert!(!ok("a/b"));
        assert!(!ok(""));
    }

    // ── DSL 解析 ──

    #[test]
    fn dsl_parse_empty_body() {
        let (steps, rollback) = parse_skill_steps("").unwrap();
        assert!(steps.is_empty());
        assert!(rollback.is_empty());
    }

    #[test]
    fn dsl_parse_single_step() {
        let body = "## Step 1: 列出任务\nlist_tasks({})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(rb.is_empty());
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].title, "列出任务");
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
    }

    #[test]
    fn dsl_parse_multiple_steps_preserves_order() {
        let body = "## Step 1: 第一步\nfoo({})\n\n## Step 2: 第二步\nbar({\"x\": \"y\"})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].tool_name, "foo");
        assert_eq!(steps[1].index, 2);
        assert_eq!(steps[1].tool_name, "bar");
        assert_eq!(steps[1].args_json, "{\"x\": \"y\"}");
    }

    #[test]
    fn dsl_parse_with_rollback_section() {
        let body = "## Step 1: 行动\ndo({\"k\": \"v\"})\n\n## Rollback\nundo({\"k\": \"v\"})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(rb.len(), 1);
        assert_eq!(rb[0].tool_name, "undo");
        assert_eq!(rb[0].args_json, "{\"k\": \"v\"}");
    }

    #[test]
    fn dsl_parse_strips_frontmatter() {
        let body = "---\nname: test\ndescription: 解析测试\n---\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_ignores_free_text_comments() {
        // 自由文字 / 注释行应被忽略，不破坏解析
        let body = "# 标题\n\n这是自由文字说明\n\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_tool_call_rejects_bad_name() {
        // 工具名含非法字符（不是 ASCII 字母数字下划线）应被拒
        assert!(parse_tool_call("foo-bar({})").is_none());
        assert!(parse_tool_call("foo bar({})").is_none());
        // 不以 ) 结尾也应被拒
        assert!(parse_tool_call("foo({}").is_none());
        assert!(parse_tool_call("foo{}").is_none());
        // 合法 name（含数字）应通过
        assert!(parse_tool_call("foo123({})").is_some());
    }

    // ── 样板 Skill 端到端 DSL 验证 ──

    /// 样板 SKILL.md 的 body（与 target/debug/skills/minimax-task-archive-demo/SKILL.md 同源）
    const DEMO_BODY: &str = "---\n\
name: minimax-task-archive-demo\n\
description: Phase 3 样板\n\
---\n\
# 任务归档演示\n\
\n\
演示 2 步 DSL。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn demo_skill_parses_two_steps_no_rollback() {
        // 样板 Skill 应被解析为 2 个 step，无 rollback 段
        let (steps, rollback) = parse_skill_steps(DEMO_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(rollback.is_empty());
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn demo_skill_substitution_chain_resolves_step1_id() {
        // 模拟调度器跑完 Step 1 → Step 2 之前的 ctx 状态：
        // Step 1 工具返回文本含 UUID → extract_task_id 拿到 id → push 进 ctx
        // Step 2 执行前调 substitute_vars → ${step1.id} 应被替换成真实 UUID
        let step1_text = "当前 3 张任务卡：\n\
- 7c9e6679-7425-40de-944b-e07fc1f90ae7 买牛奶\n\
- 11111111-2222-3333-4444-555555555555 写报告\n\
- 66666666-7777-8888-9999-000000000000 周会准备";
        let step1_uuid = extract_task_id(step1_text).unwrap();
        assert_eq!(step1_uuid, "7c9e6679-7425-40de-944b-e07fc1f90ae7");

        let ctx = vec![CompletedStep {
            index: 1,
            title: "列出当前所有任务".into(),
            result: step1_text.into(),
            id: Some(step1_uuid.clone()),
            // parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        // Step 2 的 args_json 模板（与 SKILL.md 一致）
        let step2_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(step2_args, &ctx);
        assert_eq!(resolved, format!(r#"{{"id": "{}"}}"#, step1_uuid));
    }

    // ── v2 升级样板 ──

    /// v2 SKILL.md 的 body（与 target/debug/skills/minimax-task-summary-v2/SKILL.md 同源）
    const V2_BODY: &str = "---\n\
name: minimax-task-summary-v2\n\
description: v2 升级\n\
rollback: auto\n\
---\n\
# 任务汇总 v2\n\
\n\
v2 相对 v1 改进：加 rollback 段。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n\
\n\
## Rollback\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn v2_parses_two_steps_one_rollback() {
        // v2 应被解析为 2 step + 1 rollback step
        let (steps, rollback) = parse_skill_steps(V2_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(rollback.len(), 1);
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
        // rollback 段也是 query_single_task，与主 step 复用同样变量替换路径
        assert_eq!(rollback[0].tool_name, "query_single_task");
        assert_eq!(rollback[0].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn v2_rollback_step_args_also_go_through_substitute_vars() {
        // rollback 段 args 也走变量替换
        // 模拟 Step 1 跑完后，回滚段用 ctx 里的 id 替换 ${step1.id}
        let ctx = vec![CompletedStep {
            index: 1,
            title: "list".into(),
            result: "r".into(),
            id: Some("uuid-99".into()),
            // parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        let rb_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(rb_args, &ctx);
        assert_eq!(resolved, r#"{"id": "uuid-99"}"#);
    }

    // ── 扫所有 Skill 目录通用验证 ──

    /// 扫 target/debug/skills/ 下所有 SKILL.md，验证 parse_skill_steps 通过
    /// 且至少 1 step，rollback 段（若有）也合法。
    /// 不依赖具体某个 Skill 文件——sub-agent 写完任意 Skill 后，这个测试自动覆盖。
    #[test]
    fn scan_all_skills_in_debug_dir_parse_correctly() {
        let skill_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("skills");
        let mut found = 0;
        let read_dir = match std::fs::read_dir(&skill_dir) {
            Ok(d) => d,
            Err(_) => {
                // 目录不存在不报错（开发期可能未初始化）
                eprintln!("skill dir 不存在，跳过扫瞄：{}", skill_dir.display());
                return;
            }
        };
        for entry in read_dir.flatten() {
            let skill_md = entry.path().join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let body = std::fs::read_to_string(&skill_md)
                .unwrap_or_else(|e| panic!("read {} failed: {}", skill_md.display(), e));
            // interactive 技能（如 minimax-docx）没有 DSL Step 段——
            // 它由 LLM 驱动、不进调度器，DSL 校验只针对 mode: auto 的技能
            if parse_meta(&body, "smoke").mode != "auto" {
                continue;
            }
            let (steps, rollback) = parse_skill_steps(&body).unwrap_or_else(|e| {
                panic!(
                    "parse_skill_steps failed for {}: {}",
                    entry.path().display(),
                    e
                )
            });
            assert!(!steps.is_empty(), "{} 没有 step", entry.path().display());
            for (i, step) in steps.iter().enumerate() {
                assert!(
                    !step.tool_name.is_empty(),
                    "{} step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            for (i, rb) in rollback.iter().enumerate() {
                assert!(
                    !rb.tool_name.is_empty(),
                    "{} rollback step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            found += 1;
        }
        // 不强制 Skill 数 — 任意现存 Skill 都应被正确解析
        if found == 0 {
            // 目录存在但为空（cargo clean 误删 dev mock / dev 首次未 init）—— graceful skip
            eprintln!("skill dir 为空，跳过扫瞄：{}", skill_dir.display());
            return;
        }
        eprintln!("扫瞄 {} 个 Skill，全部解析通过", found);
    }

    /// `rollback: auto` 但正文没有 `## Rollback` 段 → 降为 `none`。
    ///
    /// SKILL-DSL 承诺过这个降级，实现此前没做：声明了 auto 却一节回滚都没有，
    /// 读起来却像「会回滚」。
    #[test]
    fn rollback_auto_without_a_rollback_section_downgrades_to_none() {
        let m = parse_meta("---\nrollback: auto\n---\n# 步骤\n", "d");
        assert_eq!(m.rollback, "auto", "frontmatter 单独解析时不该改判");

        // 正文没有回滚段 → 降为 none
        let downgraded = normalize_rollback_meta(m.clone(), "## Step 1: a\nquery_tasks({})\n");
        assert_eq!(downgraded.rollback, "none", "没有回滚段可跑 = 没有回滚");

        // 有回滚段 → 保持 auto
        let kept = normalize_rollback_meta(
            m.clone(),
            "## Step 1: a\nquery_tasks({})\n\n## Rollback\ndelete_task({})\n",
        );
        assert_eq!(kept.rollback, "auto");

        // 中文标题的段落也认（判据单源在 is_rollback_heading）
        let kept_cn = normalize_rollback_meta(
            m,
            "## Step 1: a\nquery_tasks({})\n\n## 回滚\ndelete_task({})\n",
        );
        assert_eq!(kept_cn.rollback, "auto");
    }
}
