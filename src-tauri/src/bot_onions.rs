//! per-onion 配置（两层配置的文件系统层）
//!
//! 两层分工：DB 只存「指向哪份配置」与会话级覆盖（`bot_sessions` 两列），
//! 大文本（role / allowed / hooks）一律落 `<数据目录>/onions/<id>.yaml`。
//! 理由：会话列表每次整表读进内存，且备份/导出语义会因大文本入库而变复杂。
//!
//! **唯一生效规则：effective = profile 白名单 ∩ yaml.allowed**，只有收紧、没有
//! 放宽，且没有第二张 deny 列表。交集算出来可能为空——那必须让人看得见
//! （见 `render_effective_note`），「配置看着生效、实际落空」是最该挡住的失败。
//!
//! 加载时机：会话创建/切换时读一次，缓存在进程内；文件变更不热重载
//! （与既有 `BotConfig` 启动时读的语义一致）。
//!
//! `onion_id` 只是**配置模板名**，不是运行时身份——身份沿用 `session_id`，
//! 不引入第二套标识（与本仓把工具数收敛到 `BUILTIN_TOOL_COUNT` 同一原则）。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::db::SubagentProfile;

/// 配置文件名闸：仅 ASCII 字母/数字 + `-` / `_`，非空。
///
/// 与 Skill 名同款字符集（同款防路径穿越闸）。这个名字会直接拼进
/// `<数据目录>/onions/<id>.yaml`，放开 `../` 就等于让一份配置读到目录外。
pub(crate) const ONION_NAME_CHARS_OK: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';

pub fn validate_onion_name(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(ONION_NAME_CHARS_OK) {
        return Err(format!("配置名「{name}」无效（仅允许字母/数字/-/_）"));
    }
    Ok(())
}

/// 一份 onion 配置的内存形态。
///
/// `hooks` 的值是**说明串**：空串 = 该层不启用（不是「配了个空钩子」）。
/// 这里只做「哪些层被显式关掉」的读取，真正跑钩子的注册表在 `bot::hooks`。
#[derive(Debug, Clone, Deserialize)]
pub struct OnionConfig {
    pub onion_id: String,
    #[serde(default)]
    pub role: String,
    pub profile: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub max_turn: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub hooks: BTreeMap<String, String>,
}

impl OnionConfig {
    pub fn profile_enum(&self) -> Option<SubagentProfile> {
        match self.profile.as_str() {
            "research" => Some(SubagentProfile::Research),
            "coder" => Some(SubagentProfile::Coder),
            "general" => Some(SubagentProfile::General),
            _ => None,
        }
    }

    /// 该配置显式关掉的钩子层（`hooks: <layer>: ""`）。
    ///
    /// 返回 `&'static str` 形式的层名，便于喂给注册表的按层禁用；
    /// 未知层名不在此报错——那是「配置写了但运行时不认」，由
    /// `validate` 统一报，避免加载期就把整份配置判死。
    pub fn disabled_layers(&self) -> Vec<&str> {
        self.hooks
            .iter()
            .filter(|(_, v)| v.trim().is_empty())
            .map(|(k, _)| k.as_str())
            .collect()
    }
}

/// 钩子层名白名单：拼错层名（`post_tunr`）在这里报错。
///
/// 与注册表的 `HookLayer` 是**两份真相且有意不同**：这里管「配置不许写错层名」
/// （设计 §3.8 的层全集），注册表管「哪些层真有接线」。两者由
/// `design_layers_match_registry_plus_not_yet_wired` 单测钉在一起。
pub const DESIGN_HOOK_LAYERS: &[&str] = &["pre_ingest", "pre_tool", "post_tool", "post_turn"];

/// 设计里有、但尚未接线的层。期 5 的 L0 落地后从这里移走。
///
/// 单独列出而不是隐在 `DESIGN_HOOK_LAYERS - wired_names` 里算差集：差集算出来
/// 是「碰巧还没做」，而这里是「有意还没做」，两者混同会让人以为漏了。
pub const NOT_YET_WIRED_LAYERS: &[&str] = &["pre_ingest"];

/// 配置校验：工具名必须能在 `TOOLS_TABLE` 查到、profile 名要认识、层名要在白名单里。
///
/// 这是「yaml 里的名字必须能在 TOOLS_TABLE 查到」那条硬约束的落地点。
/// 报错必须点名**是哪个字段的哪个名字**——一份二十行的 yaml 被人手改错时，
/// 只说「配置无效」等于没给线索。
pub fn validate(cfg: &OnionConfig) -> Result<(), String> {
    if cfg.profile_enum().is_none() {
        return Err(format!(
            "配置 {}：profile「{}」不认识（可选 research / coder / general）",
            cfg.onion_id, cfg.profile
        ));
    }
    for name in &cfg.allowed {
        if !crate::bot::registry::tools_index().contains_key(name.as_str()) {
            return Err(format!(
                "配置 {}：allowed 里的「{name}」不在内置工具表 TOOLS_TABLE 里",
                cfg.onion_id
            ));
        }
    }
    for layer in cfg.hooks.keys() {
        if !DESIGN_HOOK_LAYERS.contains(&layer.as_str()) {
            return Err(format!(
                "配置 {}：hooks 里的层「{layer}」不存在（可选 {}）",
                cfg.onion_id,
                DESIGN_HOOK_LAYERS.join(" / ")
            ));
        }
    }
    if cfg.max_turn == Some(0) {
        // 0 轮 = 模型循环一次都不跑，子 agent 交回一句空结论。
        // 这是「配置看着生效、实际落空」的一种，必须在加载期就报错。
        return Err(format!(
            "配置 {}：max_turn 不能为 0（那等于一次工具循环都不给）",
            cfg.onion_id
        ));
    }
    Ok(())
}

/// 解析 + 校验一份配置文本（纯函数，不碰文件系统）。
///
/// 解析失败与校验失败**分开报错**：前者是 YAML 语法错（`yaml_serde` 会带
/// 行号），后者是「语法没错但配置不合法」，两者要给的指引完全不同。
pub fn parse(text: &str) -> Result<OnionConfig, String> {
    let cfg: OnionConfig =
        yaml_serde::from_str(text).map_err(|e| format!("配置 YAML 解析失败：{e}"))?;
    validate(&cfg)?;
    Ok(cfg)
}

/// `<数据目录>/onions/<id>.yaml`
pub fn onions_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    crate::db::paths::data_dir(app).join("onions")
}

/// 内置配置档（`include_str!` 编进二进制）。
///
/// 走 `include_str!` 而不是 tauri bundle 的 `resources`：`resources` 在
/// `tauri dev` 下不落到 `resource_dir()`，配置档就只在打包后才存在，
/// 开发期与单测期一律拿不到；编进二进制则 dev / test / 打包三种场景同源。
/// 样例硬约束单测已逐个 `include_str!` 钉住，改名/挪位会直接编译失败。
const BUILTIN_CONFIGS: &[(&str, &str)] = &[
    ("probe", include_str!("../resources/onions/probe.yaml")),
    (
        "writereditor",
        include_str!("../resources/onions/writereditor.yaml"),
    ),
    (
        "validator",
        include_str!("../resources/onions/validator.yaml"),
    ),
];

/// 内置配置档名（播种清单的那一份）。
///
/// 单独给一个读取口：spawn 的 schema 描述里要写出有哪些配置档可派发，
/// 那是**人手写的常量**，靠一条单测钉住它与播种清单不会漂。
pub fn builtin_config_ids() -> impl Iterator<Item = &'static str> {
    BUILTIN_CONFIGS.iter().map(|(id, _)| *id)
}

/// 启动时把内置配置档落进 `<数据目录>/onions/`（缺失才写）。
///
/// **不覆盖已存在的文件**：用户手改过的配置档被内置版本盖掉等于静默丢用户
/// 编辑，而这份配置的唯一消费者就是用户自己。内置版本随程序升级更新，
/// 想跟新版就自己改回来——这是显式的取舍。
///
/// 失败只审计不阻断启动：per-onion 配置是可选增强，写不进去只让这份功能
/// 不可用（同 `bot_skills::materialize_at_startup` 口径）。
pub fn materialize_builtin_configs<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let dir = onions_dir(app);
    for outcome in materialize_into(&dir) {
        crate::bot::audit_log(
            app,
            &match outcome {
                MaterializeOutcome::Written(id) => format!("bot_onions materialized | {id}"),
                MaterializeOutcome::DirFailed(e) => format!(
                    "bot_onions mkdir_fail | {} | {e}，本轮未播种",
                    dir.display()
                ),
                MaterializeOutcome::Failed(id, e) => {
                    format!("bot_onions write_fail | {id} | {e}")
                }
            },
        );
    }
}

enum MaterializeOutcome {
    Written(&'static str),
    /// 建目录失败 → 本轮一份都没写
    DirFailed(std::io::Error),
    Failed(&'static str, std::io::Error),
}

/// 真正的写盘（返回结果，不记审计）——审计在调用点，好让失败原因统一走
/// `bot::audit_log`。拆出来是为了能对着临时目录单测，不用真 AppHandle。
fn materialize_into(dir: &std::path::Path) -> Vec<MaterializeOutcome> {
    let mut out = Vec::new();
    if let Err(e) = std::fs::create_dir_all(dir) {
        out.push(MaterializeOutcome::DirFailed(e));
        return out;
    }
    for (id, text) in BUILTIN_CONFIGS {
        let path = dir.join(format!("{id}.yaml"));
        if path.exists() {
            continue;
        }
        match std::fs::write(&path, text) {
            Ok(()) => out.push(MaterializeOutcome::Written(id)),
            Err(e) => out.push(MaterializeOutcome::Failed(id, e)),
        }
    }
    out
}

/// 读一份配置。名字先过字符集闸（防路径穿越），再读文件、解析、校验。
pub fn load<R: tauri::Runtime>(app: &tauri::AppHandle<R>, id: &str) -> Result<OnionConfig, String> {
    validate_onion_name(id)?;
    let path = onions_dir(app).join(format!("{id}.yaml"));
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取配置 {id}.yaml 失败：{e}"))?;
    parse(&text)
}

/// 实际生效的工具名 = profile 白名单 ∩ yaml.allowed（只有收紧）。
///
/// `allowed` 为空表示「不额外收窄」，取 profile 原样——否则一份只想调
/// max_turn 的配置会静默把工具清零。
pub fn effective_tools(profile: SubagentProfile, allowed: &[String]) -> Vec<&'static str> {
    let base = crate::bot::registry::profile_whitelist(profile);
    base.iter()
        .copied()
        .filter(|t| allowed.is_empty() || allowed.iter().any(|a| a == t))
        .collect()
}

/// 交集为空时给模型/人看的一句话（§3.4：「配置看着生效、实际落空」必须可见）。
pub fn render_effective_note(profile: SubagentProfile, allowed: &[String]) -> String {
    let base = crate::bot::registry::profile_whitelist(profile);
    let eff = effective_tools(profile, allowed);
    if eff.is_empty() {
        return format!(
            "[⚠️ 配置收窄后无可用工具] profile={profile:?} 给 {len} 个，yaml.allowed 收窄到 0 个——本次不会有任何工具可用，请检查 allowed",
            len = base.len()
        );
    }
    format!(
        "[配置已生效] profile={profile:?} {len} 个工具，yaml.allowed 收窄后实际可用：{names}",
        len = eff.len(),
        names = eff.join("、")
    )
}

/// 注入子 agent system 提示词的配置段：角色 + **交集后**的实际可用工具清单。
///
/// 与 `render_effective_note` 的分工：那个写审计日志给人看，这个进提示词给
/// 模型看。必须是交集后的名单——`allowed` 是期望集，生效值是
/// `profile ∩ allowed`；把 `allowed` 原样念给模型，它会把被交集收掉的工具
/// 当成可用反复尝试，每次都撞 dispatch 白名单闸。§3.4「配置看着生效、
/// 实际落空」要连模型一起看得见，提示词这半就得给交集。
pub fn render_prompt_note(cfg: &OnionConfig, profile: SubagentProfile) -> String {
    let eff = effective_tools(profile, &cfg.allowed);
    let tools = if eff.is_empty() {
        "（无：本次没有任何工具可用。不要尝试调用任何工具，说明阻塞后直接交付你已知的部分。）"
            .to_string()
    } else {
        eff.join("、")
    };
    let role = cfg.role.trim();
    if role.is_empty() {
        format!(
            "本次运行配置档「{id}」：实际可用工具只有：{tools}。其余工具会被服务端拒绝，不要尝试调用。",
            id = cfg.onion_id
        )
    } else {
        format!(
            "本次运行配置档「{id}」：{role}。实际可用工具只有：{tools}。其余工具会被服务端拒绝，不要尝试调用。",
            id = cfg.onion_id
        )
    }
}

/// 进程内会话 → 已解析配置的缓存。
///
/// 为什么是缓存而不是每次现读：`tools_json_for` 在模型循环热路径上、且**拿不到
/// AppHandle**（它只收 session_id），读 DB 与读文件都得有 app。缓存同时对齐
/// 「文件变更不热重载」的口径（与既有 `BotConfig` 启动时读一致）——改完 yaml
/// 要新建会话才生效，这是有意为之，不是 bug。
static SESSION_CFG: OnceLock<Mutex<HashMap<String, Option<OnionConfig>>>> = OnceLock::new();

fn session_cfg() -> &'static Mutex<HashMap<String, Option<OnionConfig>>> {
    SESSION_CFG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取某会话已绑定的配置（缓存未绑定过 = 没有配置 → None）。
///
/// 锁中毒不阻断工具调用：取回内层数据继续用，缓存丢一次只是退回默认权限，
/// 不该让一条坏缓存把模型循环整个卡住。
pub fn cached_for_session(session_id: Option<&str>) -> Option<OnionConfig> {
    let sid = session_id?;
    let map = session_cfg().lock().unwrap_or_else(|e| e.into_inner());
    map.get(sid).cloned().flatten()
}

/// 会话创建/切换时调一次：读 DB 元列 + 读 yaml + 解析校验，结果进缓存。
///
/// 读不到配置（没绑 / 文件不在 / 解析失败）都记 None 而不是报错中断——绑定是
/// 可选增强，坏配置不该让会话开不起来；失败原因已由 `load` 的错误文本带出。
pub fn bind_session<R: tauri::Runtime>(app: &tauri::AppHandle<R>, session_id: &str) {
    let loaded = match super::db::open_db(app) {
        Ok(conn) => match super::db::bot_sessions::bot_session_onion(&conn, session_id) {
            Ok(Some(meta)) => load(app, &meta.config).ok(),
            Ok(None) => None,
            Err(e) => {
                crate::bot::audit_log(
                    app,
                    &format!("bot_onions read_meta_fail | {} | {e}", session_id),
                );
                None
            }
        },
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!("bot_onions db_open_fail | {} | {e}", session_id),
            );
            None
        }
    };
    if let Some(cfg) = &loaded {
        crate::bot::audit_log(
            app,
            &format!(
                "bot_onions bound | session={session_id} config={} | {}",
                cfg.onion_id,
                render_effective_note(
                    cfg.profile_enum().unwrap_or(SubagentProfile::General),
                    &cfg.allowed
                )
            ),
        );
        // 配置里 `hooks: <layer>: ""` = 该层显式关闭，落到注册表的按层开关。
        // 只置「关」不置「开」：注册表默认全开，配置没提到的层一律照常跑。
        let reg = crate::bot::hooks::registry();
        for layer in cfg.disabled_layers() {
            match crate::bot::hooks::HookLayer::from_name(layer) {
                Some(l) => reg.disable_layer_for_session(l, session_id),
                None => crate::bot::audit_log(
                    app,
                    &format!(
                        "bot_onions layer_not_wired | session={session_id} layer={layer} | \
                         该层尚未接线，配置里的关闭意图本次未生效"
                    ),
                ),
            }
        }
    }
    session_cfg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.to_string(), loaded);
}

/// 测试/子 agent 换配置时用：直接塞一份已解析配置进缓存。
#[cfg(test)]
pub(crate) fn bind_for_test(session_id: &str, cfg: Option<OnionConfig>) {
    session_cfg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.to_string(), cfg);
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
onion_id: validator
role: 公文合规校验，只读
profile: coder
allowed: [read_text_file, grep_files, run_python, ast_inspect, write_artifact_file]
max_turn: 4
temperature: 0
hooks:
  pre_tool: ""
"#;

    #[test]
    fn name_gate_blocks_path_traversal() {
        assert!(validate_onion_name("probe").is_ok());
        assert!(validate_onion_name("writer-editor_2").is_ok());
        assert!(validate_onion_name("").is_err());
        assert!(validate_onion_name("../etc").is_err(), "上跳必须被拒");
        assert!(validate_onion_name("a/b").is_err(), "斜杠必须被拒");
        assert!(validate_onion_name("probe.yaml").is_err(), "扩展名必须被拒");
    }

    #[test]
    fn parses_the_canonical_validator_shape() {
        let cfg = parse(VALID).expect("样例应可解析");
        assert_eq!(cfg.onion_id, "validator");
        assert_eq!(cfg.profile_enum(), Some(SubagentProfile::Coder));
        assert_eq!(cfg.allowed.len(), 5);
        assert_eq!(cfg.max_turn, Some(4));
        assert_eq!(cfg.temperature, Some(0.0));
        assert_eq!(cfg.disabled_layers(), vec!["pre_tool"]);
    }

    #[test]
    fn empty_hook_value_means_disabled_not_configured() {
        let cfg = parse(VALID).unwrap();
        assert_eq!(cfg.disabled_layers(), vec!["pre_tool"]);
        // 非空说明串 = 该层照常
        let on = parse("onion_id: p\nprofile: coder\nhooks:\n  post_tool: 清洗产物\n").unwrap();
        assert!(on.disabled_layers().is_empty());
    }

    // ── 期 0 的硬约束：yaml 里的名字必须能在 TOOLS_TABLE 查到 ──

    #[test]
    fn unknown_tool_name_is_rejected_naming_the_offending_entry() {
        let err = parse("onion_id: p\nprofile: coder\nallowed: [read_text_file, ast_inspectt]\n")
            .expect_err("拼错的名字必须被拒");
        assert!(err.contains("ast_inspectt"), "报错要点名是哪个名字：{err}");
        assert!(err.contains("TOOLS_TABLE"), "报错要指出去处：{err}");
    }

    #[test]
    fn unknown_profile_is_rejected() {
        let err = parse("onion_id: p\nprofile: writereditor\n").expect_err("未知 profile 必须被拒");
        assert!(err.contains("writereditor"), "{err}");
    }

    #[test]
    fn unknown_hook_layer_is_rejected_not_silently_ignored() {
        let err = parse("onion_id: p\nprofile: coder\nhooks:\n  post_tunr: \"\"\n")
            .expect_err("手滑的层名必须被拒");
        assert!(err.contains("post_tunr"), "{err}");
    }

    #[test]
    fn syntax_error_is_reported_separately_from_invalid_config() {
        let err = parse("onion_id: [unclosed\n").expect_err("语法错必须报错");
        assert!(err.contains("YAML 解析失败"), "{err}");
    }

    // ── 权限公式：effective = profile ∩ allowed，只有收紧 ──

    #[test]
    fn effective_is_intersection_not_union() {
        let eff = effective_tools(
            SubagentProfile::Coder,
            &["read_text_file".to_string(), "edit_file".to_string()],
        );
        assert_eq!(eff, vec!["read_text_file", "edit_file"]);
        // profile 里有、allowed 里没有的必须被收掉
        assert!(!eff.contains(&"run_python"));
    }

    #[test]
    fn yaml_can_never_widen_beyond_profile() {
        // validator 用 coder profile 但把写工具收掉：交集只会更小。
        // 反向情形：yaml 写了 research 没有的工具，也拿不到。
        let eff = effective_tools(
            SubagentProfile::Research,
            &["run_python".to_string(), "edit_file".to_string()],
        );
        assert!(
            eff.is_empty(),
            "yaml 不得放宽 profile：research 拿不到 run_python/edit_file，实得 {eff:?}"
        );
    }

    #[test]
    fn empty_allowed_means_no_extra_narrowing() {
        let base = crate::bot::registry::profile_whitelist(SubagentProfile::Coder);
        assert_eq!(
            effective_tools(SubagentProfile::Coder, &[]).len(),
            base.len()
        );
    }

    #[test]
    fn design_layers_match_registry_plus_not_yet_wired() {
        // 设计层全集 = 已接线层 + 明确列出的「还没做」。三份名单必须严丝合缝：
        // 少一个 = 有层名谁都不认；多一个 = 有层名被当成已接线却其实没接线。
        let mut union: Vec<&str> = crate::bot::hooks::HookLayer::wired_names().to_vec();
        union.extend(NOT_YET_WIRED_LAYERS.iter().copied());
        union.sort_unstable();
        let mut design = DESIGN_HOOK_LAYERS.to_vec();
        design.sort_unstable();
        assert_eq!(
            union, design,
            "设计层全集必须恰好等于「已接线层 ∪ 尚未接线层」"
        );
        // 尚未接线的那批，从_name 必须真的认不出来（否则「没关成」是假的）
        for name in NOT_YET_WIRED_LAYERS {
            assert!(
                crate::bot::hooks::HookLayer::from_name(name).is_none(),
                "{name} 列在尚未接线里，from_name 却认得——要么接线了要么名单过期"
            );
        }
    }

    #[test]
    fn empty_result_is_surfaced_not_hidden() {
        let note = render_effective_note(SubagentProfile::Research, &["run_python".to_string()]);
        assert!(note.contains("无可用工具"), "{note}");
        // 正常交集要给模型列出实际可用工具，不能让它以为 yaml 没生效
        let ok = render_effective_note(SubagentProfile::Coder, &["read_text_file".to_string()]);
        assert!(ok.contains("read_text_file"), "{ok}");
        assert!(ok.contains("配置已生效"), "{ok}");
    }

    // ── 期 0 硬约束：仓库里随包发布的三份配置必须自洽 ──

    /// 三份随包发布配置的原文。**用 `include_str!` 逐个点名**而不是扫目录：
    /// 目录扫描依赖测试进程 cwd，不可靠；逐个点名还顺带强制「新增一份配置必须
    /// 登记进这条测试」，不会留一份没人校验的野配置。
    const SHIPPED: &[(&str, &str)] = &[
        ("probe", include_str!("../resources/onions/probe.yaml")),
        (
            "writereditor",
            include_str!("../resources/onions/writereditor.yaml"),
        ),
        (
            "validator",
            include_str!("../resources/onions/validator.yaml"),
        ),
    ];

    #[test]
    fn shipped_configs_all_pass_the_tool_name_hard_constraint() {
        // 这条就是「yaml 里出现的每个工具名都必须能在 TOOLS_TABLE 查到」的落地点。
        // 没有它，新增内置工具时不同步核对三份 yaml，工具会被交集悄悄收掉，
        // 而配置「看着生效、实际落空」。
        for (name, text) in SHIPPED {
            let cfg = parse(text).unwrap_or_else(|e| panic!("配置 {name} 应合法：{e}"));
            assert_eq!(
                &cfg.onion_id, name,
                "文件名与 onion_id 不一致：bot_sessions 存的是文件名，\
                 对不上会让那条引用指错配置"
            );
            for tool in &cfg.allowed {
                assert!(
                    crate::bot::registry::tools_index().contains_key(tool.as_str()),
                    "配置 {name} 的 allowed 里有 {tool}，但它不在 TOOLS_TABLE"
                );
            }
        }
    }

    #[test]
    fn shipped_configs_use_disabled_layers_not_fake_hook_config() {
        // `hooks: <layer>: ""` = 该层显式关闭。层名必须是注册表真有的那一批，
        // 否则就是「配置写了但运行时不认」——validate 已经拒，这里再钉一次
        // 三份配置确实都用上了这个语义（至少有一处显式关闭）。
        let mut any = false;
        for (name, text) in SHIPPED {
            let cfg = parse(text).unwrap();
            for layer in cfg.disabled_layers() {
                assert!(
                    DESIGN_HOOK_LAYERS.contains(&layer),
                    "配置 {name} 引用了不存在的层 {layer}"
                );
                any = true;
            }
        }
        assert!(
            any,
            "三份配置至少应有一处显式关闭的层，否则 hooks 语义无处落地"
        );
    }

    #[test]
    fn validator_config_keeps_coder_profile_but_drops_write_tools() {
        // 「只能收紧」模型的样板用例：profile 给上限，yaml 决定本次不用什么。
        let (_, text) = SHIPPED
            .iter()
            .find(|(n, _)| *n == "validator")
            .expect("validator 配置存在");
        let cfg = parse(text).unwrap();
        assert_eq!(cfg.profile_enum(), Some(SubagentProfile::Coder));
        let eff = effective_tools(cfg.profile_enum().unwrap(), &cfg.allowed);
        assert!(!eff.contains(&"edit_file"), "validator 不得拿到写文件");
        assert!(!eff.contains(&"write_file"), "validator 不得拿到写文件");
        assert!(
            eff.contains(&"ast_inspect"),
            "validator 必须能跑 AST 静态检查"
        );
    }

    #[test]
    fn writereditor_does_not_get_ast_inspect() {
        // v1 有意不给：撰写者按大纲写，不需要符号级结构查询。
        let (_, text) = SHIPPED
            .iter()
            .find(|(n, _)| *n == "writereditor")
            .expect("writereditor 配置存在");
        let cfg = parse(text).unwrap();
        let eff = effective_tools(cfg.profile_enum().unwrap(), &cfg.allowed);
        assert!(!eff.contains(&"ast_inspect"));
        assert!(eff.contains(&"write_file"), "撰写者要能写文件");
    }

    // ── 首次启动播种（期 6：三份配置从此在生产可达）──

    #[test]
    fn materialize_writes_all_builtins_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "wm_onion_mat_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        // ① 空目录 → 三份全部写入，且落盘内容与内置源逐字一致
        let first = materialize_into(&dir);
        assert_eq!(
            first.len(),
            BUILTIN_CONFIGS.len(),
            "首启应写入全部内置配置档，实际 {:?}",
            first
                .iter()
                .map(|o| match o {
                    MaterializeOutcome::Written(id) => *id,
                    MaterializeOutcome::DirFailed(_) => "<mkdir_fail>",
                    MaterializeOutcome::Failed(id, _) => id,
                })
                .collect::<Vec<_>>()
        );
        for (id, text) in BUILTIN_CONFIGS {
            let got = std::fs::read_to_string(dir.join(format!("{id}.yaml"))).unwrap();
            assert_eq!(&got, text, "{id} 落盘内容应与内置源逐字一致");
        }
        // ② 已存在 → 一份都不重写（用户手改的配置档不能被内置版盖掉）
        let edited = "onion_id: probe\nrole: 我改过的\nprofile: research\n";
        std::fs::write(dir.join("probe.yaml"), edited).unwrap();
        assert!(
            materialize_into(&dir).is_empty(),
            "已存在的配置档不得被覆盖"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("probe.yaml")).unwrap(),
            edited,
            "用户改过的配置档必须原样保留"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_configs_are_parseable_under_materialization_path() {
        // 播种写出去的必须是能通过 load 的配置：materialize 只拷文本，
        // 语法/字段不合法要等到派发时才炸——那时已经建了会话。
        for (id, text) in BUILTIN_CONFIGS {
            let cfg = parse(text).unwrap_or_else(|e| panic!("内置配置 {id} 不合法：{e}"));
            assert_eq!(&cfg.onion_id, id, "文件名与文件内 onion_id 应一致");
        }
    }

    // ── 提示词段（§3.4「配置看着生效、实际落空」要让模型也看得见）──

    #[test]
    fn prompt_note_lists_intersection_not_the_allowed_list() {
        // allowed 里有 profile 给不了的工具时，提示词绝不能把它念进「可用」——
        // 那会让模型反复调一个必然被闸拒的工具。
        let cfg = parse(VALID).unwrap();
        let profile = cfg.profile_enum().unwrap();
        let note = render_prompt_note(&cfg, profile);
        for tool in effective_tools(profile, &cfg.allowed) {
            assert!(
                note.contains(tool),
                "交集内的 {tool} 应出现在提示词里：{note}"
            );
        }
        let base = crate::bot::registry::profile_whitelist(profile);
        let dropped = base
            .iter()
            .find(|t| !effective_tools(profile, &cfg.allowed).contains(t))
            .expect("VALID 的 allowed 应收窄掉至少一个工具");
        assert!(
            !note.contains(*dropped),
            "被交集收掉的 {dropped} 不得出现在「实际可用」里：{note}"
        );
        assert!(
            note.contains(cfg.role.trim()),
            "配置档 role 应进提示词（否则 role 字段无消费者）：{note}"
        );
    }

    #[test]
    fn prompt_note_says_no_tools_when_narrowed_to_zero() {
        // allowed 里的工具全在 TOOLS_TABLE 但全不在 research 白名单内
        // → 交集为空。这正是「配置看着生效、实际落空」的最坏形态。
        let cfg = parse("onion_id: probe\nprofile: research\nallowed: [edit_file, write_file]\n")
            .unwrap();
        assert!(
            effective_tools(SubagentProfile::Research, &cfg.allowed).is_empty(),
            "本用例的前提是交集确实为空，否则断言无效"
        );
        let note = render_prompt_note(&cfg, SubagentProfile::Research);
        assert!(
            note.contains("没有任何工具可用"),
            "交集为空时提示词要明说，否则模型会以为工具丢了：{note}"
        );
        assert!(
            !note.contains("edit_file"),
            "空交集不得还把 allowed 原样念成可用：{note}"
        );
    }

    #[test]
    fn validate_rejects_zero_max_turn() {
        let e = parse("onion_id: z\nprofile: coder\nmax_turn: 0\n").unwrap_err();
        assert!(
            e.contains("max_turn"),
            "max_turn=0 应被加载期拒掉（等于一次循环都不给）：{e}"
        );
    }
}
