//! 前后端共享常量单一来源。
//!
//! Rust 为唯一真相：前端启动时经 `app_consts` 命令拉取并缓存（src/lib/consts.ts），
//! 避免前后端手写两份漂移。新增共享常量时在此加字段并同步前端 AppConsts 类型。

use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AppConsts {
    pub max_task_files: usize,
    pub max_title: usize,
    pub max_note: usize,
    pub image_exts: Vec<&'static str>,
}

#[tauri::command]
pub fn app_consts() -> AppConsts {
    AppConsts {
        max_task_files: crate::db::MAX_TASK_FILES,
        max_title: crate::bot::MAX_TITLE,
        max_note: crate::bot::MAX_NOTE,
        image_exts: crate::bot_chat::IMAGE_EXTS.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_match_source_consts() {
        let c = app_consts();
        assert_eq!(c.max_task_files, crate::db::MAX_TASK_FILES);
        assert_eq!(c.max_title, crate::bot::MAX_TITLE);
        assert_eq!(c.max_note, crate::bot::MAX_NOTE);
        assert_eq!(c.image_exts, crate::bot_chat::IMAGE_EXTS);
    }

    #[test]
    fn wire_field_names_locked() {
        // 前端按 snake_case 字段名解析，改名必须两侧同步
        let v = serde_json::to_value(app_consts()).unwrap();
        for k in ["max_task_files", "max_title", "max_note", "image_exts"] {
            assert!(v.get(k).is_some(), "缺字段 {k}");
        }
    }
}
