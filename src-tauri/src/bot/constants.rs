//! Agent 风控工程常量（开发者维护，设置页不暴露、不参与用户配置）。
//!
//! 与 `params.rs` 的分工：
//! - `params.rs` = **用户可配**的那些（可编辑区）+ 生效值解析；
//! - 本文件 = **开发者维护**的那些（硬编码只读区），改这里 = 改风控策略。
//!
//! 收拢本文件的理由：这些常量原先散落在 `bot_model_loop/`、`bot_skills/`、
//! `bot_fs.rs`、`bot_archive.rs` 等多处，每处都带一句「这是安全上限」的注释，
//! 但没有任何一处能一眼看全「当前的风控策略是什么」。风控参数是要被整体审阅
//! 的（改一个上限前得知道另外几个上限是多少），散着放审不动。
//!
//! 分三段：上下文/IO 闸、熔断阶梯、流程风控。段内注释只写「为什么是这个数」，
//! 不写决策日期与工单号——那些属于 DEVLOG。

// ── 上下文与 IO 闸 ──

/// 上下文裁剪滞回比例（百分比）的可取区间：超预算时一次裁到预算的这个比例。
///
/// 留余量稳住隐式前缀缓存——贴边裁会让长会话几乎每轮都改一次缓存前缀。
/// 下界再低就是历史断崖式变短，摘要都救不回来；上界再高等于没有余量，
/// 贴边裁的问题原样回来。
pub const HISTORY_TRUNCATE_TARGET_PCT_MIN: u32 = 50;
pub const HISTORY_TRUNCATE_TARGET_PCT_MAX: u32 = 90;

/// 单次请求 msgs 总字符硬顶：请求体压缩阶梯（软压 70% / 硬压 85% / Fuse 100%）
/// 的基数。
///
/// 与会话历史预算是两层：历史预算管持久化的对话历史裁到多大，这里管一次请求
/// 连同轮内工具结果一起最多多大。中文 ≈ 1 token/字符，这个量级贴着多数模型的
/// 可用输入量。
pub const CONTEXT_CHARS_HARD_CAP: usize = 200_000;

/// 轮内工具结果滑动窗：累计超这个字符数开始裁，只保留最近若干条。
///
/// 实测单次提问的工具结果 p90 落在两万字出头，这个值取在 p90 与 p95 之间。
pub const TOOL_WINDOW_TRIGGER_CHARS: usize = 24_000;

/// 滑动窗保留的最近工具结果条数。
///
/// 少一条模型就可能忘了上一轮做过什么（重跑工具 = 重跑副作用）。
pub const TOOL_WINDOW_KEEP_RECENT: usize = 6;

/// 证据（Receipt 凭证）单条输出上限（字符）。
///
/// 凭证单条撑爆上下文比截断一条工具结果更难排查——它出现在「结论」位置，
/// 丢了就等于结论缺了一块。
pub const EVIDENCE_OUTPUT_CHARS: usize = 64 * 1024;

/// LLM 首包超时（秒）。
///
/// 首包拿不到就等于这次请求没成；再长的生成都是首包之后的流式增量，
/// 由流式空闲超时兜底。
pub const LLM_HEADER_TIMEOUT_SECS: u64 = 60;

/// 流式重试次数：首包失败最多重试这么多次。
pub const LLM_MAX_ATTEMPTS: u32 = 2;

/// 流式重试间隔（毫秒）。
///
/// 间隔太短会撞上游 500（重试本身成了压测源）。
pub const LLM_RETRY_DELAY_MS: u64 = 1500;

/// Python 沙箱单次执行超时（秒）。
pub const PYTHON_TIMEOUT_SECS: u64 = 60;

/// 文件读取单次上限（字节）。
pub const READ_MAX_BYTES: usize = 100 * 1024;
/// 文件读取默认行数。
pub const READ_DEFAULT_LINES: usize = 500;
/// 文件读取最大行数。
pub const READ_MAX_LINES: usize = 2000;
/// grep 最大命中条数。
pub const GREP_MAX_HITS: usize = 50;
/// grep 参与扫描的单文件上限（字节）。
pub const GREP_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// 目录列举最大条目数。
pub const LIST_MAX_ENTRIES: usize = 200;
/// 目录递归深度上限。
pub const WALK_MAX_DEPTH: usize = 5;
/// 可编辑文件体积上限。
pub const EDIT_MAX_FILE_BYTES: usize = 1024 * 1024;
/// 写入文件内容上限。
pub const WRITE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// 归档解压：单个文件解压后上限。
pub const UNPACK_MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// 归档解压：整包解压总量上限。
pub const UNPACK_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// 归档解压：条目数上限。
pub const UNPACK_MAX_ENTRIES: usize = 5000;
/// 归档解压：放大比上限。
pub const MAX_EXPANSION_RATIO: u64 = 200;
/// 放大比判定的地板：解压总量超过它才查比值。
pub const RATIO_FLOOR_BYTES: u64 = 64 * 1024 * 1024;
/// 归档打包：单个文件上限。
pub const PACK_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// 归档打包：整包总量上限。
pub const PACK_MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 归档打包：条目数上限。
pub const PACK_MAX_ENTRIES: usize = 20_000;
// ── 熔断阶梯 ──
//
// 三段式：软告警 → 收尾窗 → 硬熔断。分段的意义是给模型「自己停下来」的
// 机会——硬熔断一刀切时，正在跑的工具、已经说出口的正文、已落盘的产物一起
// 悬空，产出归零。
//
// 阶梯按**预算占比**而不是绝对次数定义，档位变化时阶梯位置自动跟着走。

/// 软告警阈值（预算占比，百分比）：达到即提醒模型停止展开新步骤。
pub const FUSE_SOFT_WARN_PCT: u32 = 90;

/// 收尾窗阈值（预算占比，百分比）：达到即进入收尾窗，只放行写产物类工具。
pub const FUSE_WRAP_UP_PCT: u32 = 95;

/// 硬熔断后额外放行的写文件裕量（次）。
///
/// 收尾窗里写产物类调用不占预算计数，这份额量是「收尾窗都没能让它停下」时的
/// 绝对兜底：让模型还有机会把结果写出来再停。
pub const SUBAGENT_WRAP_UP_FUSE_MARGIN: usize = 5;

/// 收尾窗放行的工具白名单：写产物 + 读自己的任务卡。
///
/// 触顶后唯一该做的事就是「把已有发现写成产物文件」——继续探索没有意义，
/// 而读回自己的卡才知道产物写到哪了。
pub const WRAP_UP_TOOLS: [&str; 4] = [
    "write_artifact_file",
    "write_file",
    "edit_file",
    "read_own_card",
];

// ── 流程风控（自研 Plan+Receipt 体系专属）──

/// RePlan 全盘重规划次数上限。
///
/// 防重规划死循环：每一轮重规划都重跑一遍计划校验，次数不限就是死循环。
pub const MAX_REPLANS: usize = 2;

/// 单份计划的步数上限。
///
/// 避免生成过度庞大、不可执行的计划——步数越多单步越粗，验收标准越糊。
pub const MAX_PLAN_STEPS: usize = 8;

/// 终稿自检的轮次间隔（轮）。
///
/// 每隔这么多轮自动触发一次终稿校验；模型提交交付时强制自检，不受间隔限制。
pub const FINAL_SELFCHECK_ROUND_GAP_DEFAULT: u32 = 8;

/// 终稿自检轮次间隔上限。
pub const FINAL_SELFCHECK_ROUND_GAP_CAP: u32 = 100;

/// Verifier 审计失败后的修复重试预算上限。
///
/// 超过这个次数还失败的任务属硬阻塞，如实汇报比继续烧 token 更省。
pub const VERIFIER_RETRY_MAX_CAP: u32 = 2;

/// 联网搜索单次返回条数上限（硬顶）。
pub const SEARCH_MAX_RESULTS_CAP: u32 = 10;
/// 联网搜索单次返回条数下限。
pub const SEARCH_MAX_RESULTS_MIN: u32 = 1;

/// ask_user 单次执行可问次数上限。
pub const ASK_BUDGET_MIN: u8 = 1;
pub const ASK_BUDGET_MAX: u8 = 5;

// ── 技能配额 ──

/// 技能步数默认值。
///
/// 步数按「非 use_skill 的工具调用」计。产出型流程（检索 → 读模板 → 写脚本
/// → 跑脚本 → 逐页精修 → 合并 → 校验）天然二三十步，默认值低于这个量级时
/// 技能会在做一半时被步数闸掐掉，而此时产物往往已经落盘。
pub const SKILL_DEFAULT_MAX_STEPS: usize = 50;

/// 技能步数声明上限。
///
/// 上限留两倍余量给特别长的流程——上限卡在默认值上时，技能写多大的声明都会
/// 被夹回同一个数，声明等于没写。
pub const SKILL_MAX_STEPS_LIMIT: usize = 100;

/// 技能墙钟默认值（秒）。
///
/// 墙钟算的是技能启动到此刻的**累计墙钟**，里面大部分是等模型往返——每步
/// 一次请求往返十几秒，多步任务的墙钟随步数线性增长。
pub const SKILL_DEFAULT_TIMEOUT_SECS: u64 = 1800;

/// 技能墙钟声明上限（秒）。
pub const SKILL_MAX_TIMEOUT_SECS_LIMIT: u64 = 3600;

/// 技能预算软警告：步数余量阈值（剩余步数 ≤ 总步数的这个比例即提醒）。
pub const SKILL_BUDGET_WARN_STEPS_DIVISOR: usize = 10;
/// 技能预算软警告：步数余量阈值钳制区间。
pub const SKILL_BUDGET_WARN_STEPS_CLAMP: (usize, usize) = (1, 5);
/// 技能预算软警告：时间余量阈值（剩余时间 ≤ 总时间的这个比例即提醒）。
pub const SKILL_BUDGET_WARN_SECS_DIVISOR: u64 = 4;
/// 技能预算软警告：时间余量阈值钳制区间（秒）。
///
/// 上下界与 `timeout_secs` 同类型（u64）：余量是「已经过了多少秒」，
/// 恒非负；有符号类型在这里只会逼着调用点做一次无谓的转换。
pub const SKILL_BUDGET_WARN_SECS_CLAMP: (u64, u64) = (5, 60);

// ── 子 agent 编排 ──

/// 子 agent 全局并发上限。
pub const SUBAGENT_MAX_GLOBAL_RUNNING: usize = 3;
/// 同一主会话同时运行的子 agent 上限。
pub const SUBAGENT_MAX_PER_SESSION_RUNNING: usize = 2;

/// `check_subagent` 单次等待上限（毫秒）。
pub const SUBAGENT_CHECK_WAIT_MS_CAP: u64 = 600_000;
