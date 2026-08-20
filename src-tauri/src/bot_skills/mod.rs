//! WMessage 机器人技能系统（轻量版 Agent Skills，参照 Anthropic SKILL.md 规范）。
//!
//! - 技能 = 数据目录 skills/<name>/SKILL.md（YAML frontmatter：name + description；正文=执行指令）
//! - Progressive disclosure：系统提示词只注入「名称+描述」清单；正文由 use_skill 工具按需读取
//! - 安装：设置页导入技能文件夹（拷贝进数据目录），或手动放入数据目录 skills/
//! - 安全：技能名白名单字符集（防路径穿越）；正文读取有大小上限

mod files;
mod manage;
mod parse;
mod runtime;
mod scheduler;
mod state;
mod vars;

pub use files::*;
pub use manage::*;
pub use parse::*;
pub use runtime::*;
pub use scheduler::*;
pub use state::*;
pub use vars::*;
