//! 老 free-form 审计入口冻结守卫：`audit_log` 已桥接到 write_event 单一写侧，
//! 存量调用点按白名单收敛（迁移完一文件摘一文件）；白名单外出现新的
//! `audit_log(` 调用即 fail——新代码一律 `audit_event!`。

const WHITELIST: &[&str] = &[
    // 定义与桥接实现本体
    "bot/config/audit.rs",
    // 存量调用文件（迁移一批摘一批；摘干净后本白名单清空、audit_log 退役）
    "apps/manage.rs",
    "bot/config/commands.rs",
    "bot/corrections.rs",
    "bot/mcp/builtin.rs",
    "bot/subagent_digest.rs",
    "bot/subagent_repair.rs",
    "bot/subagent_runner.rs",
    "bot/subagent_spawn.rs",
    "bot/subagent_tools.rs",
    "bot/mcp/commands.rs",
    "bot/tools.rs",
    "bot_fs.rs",
    "bot_chat.rs",
    "bot_desktop.rs",
    "bot_manage_file.rs",
    "bot_model_loop.rs",
    "bot_onions.rs",
    "bot_orchestrator.rs",
    "bot_reminders.rs",
    "bot_scheduler.rs",
    "bot_skills/files.rs",
    "bot_skills/manage.rs",
    "bot_skills/remote.rs",
    "bot_skills/runtime.rs",
    "bot_slash.rs",
    "bot_ui.rs",
    "bot_web.rs",
    "db/bot_history.rs",
    "due_notify.rs",
    "exec_steps.rs",
    "ocr.rs",
    "py/document.rs",
];

/// 把一个文件的「生产切片」切出来：剔除全部 `#[cfg(test)]` 块。
///
/// 旧口径是「按首个 `#[cfg(test)]` 截断」——它假设测试代码都在文件末尾。
/// 这个假设在测试模块被拆成独立文件后失效：`bot_fs.rs` 的首个
/// `#[cfg(test)]` 在第 75 行，于是它 2000 多行生产代码（含 20 处
/// `audit_log(` 调用）整段落在「测试区」里，守卫根本扫不到——扫描范围
/// 取决于代码摆放位置，而不是代码是不是测试。
///
/// 改成逐块剔除后，口径与文件布局无关：生产代码无论放在文件哪个位置
/// 都进扫描范围，测试代码无论内联还是独立成文件都不进。
fn prod_slice(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(idx) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + "#[cfg(test)]".len()..];
        // 属性后可能还跟着别的属性行与 mod 声明行
        let Some(mstart) = after.find("mod ") else {
            // 不是测试 mod（是 static / fn 等），保留原样继续
            out.push_str("#[cfg(test)]");
            out.push_str(after);
            return out;
        };
        out.push_str(&after[..mstart]);
        let tail = &after[mstart..];
        // 找到 mod 块的配对大括号，取出块体整段丢弃
        let mut depth = 0i32;
        let mut started = false;
        let mut end = tail.len();
        for (off, ch) in tail.char_indices() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => {
                    depth -= 1;
                    if started && depth <= 0 {
                        end = off + ch.len_utf8();
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 测试自留地文件：整份都是测试代码，不进扫描范围。
///
/// 判据是**模块声明**而不是文件名：父文件里 `#[cfg(test)] mod X;` 声明出来
/// 的子模块文件，整个文件只在测试构建里存在。Rust 的模块文件规则是确定的
/// ——父文件 `src/a.rs` 与 `src/a/mod.rs` 的子模块都在 `src/a/X.rs`——所以
/// 从声明就能算出子文件路径，不用猜名字。
///
/// 为什么不能用文件名猜：`audit/freeform_freeze.rs`（守卫自身）、
/// `bot/eval.rs` 都是测试专用但名字里没有 "test"；反过来
/// `bot/registry.rs` 名字像测试文件却是生产代码。猜名字两处都错。
fn test_only_files(src: &std::path::Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if !p.extension().is_some_and(|x| x == "rs") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            for name in cfg_test_mod_decls(&body) {
                // 父文件 src/a.rs 的子模块在 src/a/X.rs；
                // 父文件 src/a/mod.rs 的子模块也在 src/a/X.rs
                let child = if p.file_name().is_some_and(|n| n == "mod.rs") {
                    p.parent().map(|d| d.join(format!("{name}.rs")))
                } else {
                    p.parent().and_then(|d| {
                        p.file_stem()
                            .map(|stem| d.join(stem).join(format!("{name}.rs")))
                    })
                };
                if let Some(c) = child {
                    if let Ok(rel) = c.strip_prefix(src) {
                        out.insert(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
    }
    out
}

/// 提取一个文件里所有 `#[cfg(test)] mod X;` 声明的模块名。
///
/// 只认「声明式」形态（`mod X;`）——内联形态（`mod X { ... }`）的块体在
/// 同一个文件里，由 `prod_slice` 负责剔除，不走这条路径。
fn cfg_test_mod_decls(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let Some(next) = lines.peek() else { break };
        let trimmed = next.trim();
        if let Some(rest) = trimmed.strip_prefix("mod ") {
            let name: String = rest
                .trim()
                .trim_end_matches(';')
                .trim()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

fn scan_dir(
    dir: &std::path::Path,
    test_only: &std::collections::HashSet<String>,
    hits: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_dir(&p, test_only, hits);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let rel = p
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if test_only.contains(&rel) {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            let count = prod_slice(&body).matches("audit_log(").count();
            if count > 0 && !WHITELIST.contains(&rel.as_str()) {
                hits.push(format!("{rel}: {count} 处"));
            }
        }
    }
}

#[test]
fn audit_log_frozen_to_migration_whitelist() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let test_only = test_only_files(&src);
    let mut hits = Vec::new();
    scan_dir(&src, &test_only, &mut hits);
    assert!(
        hits.is_empty(),
        "白名单外出现 audit_log( 调用（新代码一律 audit_event!；存量迁移完从白名单摘除）：\n  {}",
        hits.join("\n  ")
    );
}

/// 口径自检：生产切片与代码位置无关。
///
/// 旧口径按「首个 `#[cfg(test)]` 截断」，于是「测试块在前、生产代码在后」
/// 的文件里，后半段生产代码整个扫不到——`bot_fs.rs` 的 20 处
/// `audit_log(` 调用就是这么躲过去的。口径退回到位置启发式时这个测试会红。
#[test]
fn prod_slice_ignores_code_position() {
    let body = "\
fn production_head() { audit_log(1); }
#[cfg(test)]
mod tests { fn t() { audit_log(2); } }
fn production_tail() { audit_log(3); }
";
    let prod = prod_slice(body);
    assert!(
        prod.contains("audit_log(1)"),
        "测试块之前的生产代码必须在扫描范围内"
    );
    assert!(
        prod.contains("audit_log(3)"),
        "测试块之后的生产代码必须在扫描范围内（旧口径在这里漏扫）"
    );
    assert!(
        !prod.contains("audit_log(2)"),
        "测试块内部的调用不属生产调用"
    );
}

/// 口径自检：测试专用文件靠模块声明识别，不靠文件名。
///
/// `audit/freeform_freeze.rs`（守卫自身）与 `bot/eval.rs` 都是测试专用
/// 但名字里没有 "test"；`bot/registry.rs` 名字像测试文件却是生产代码。
/// 靠猜名字两处都错，所以判据必须是 `#[cfg(test)] mod X;` 声明。
#[test]
fn test_only_files_resolved_from_module_declarations() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let test_only = test_only_files(&src);
    assert!(
        test_only.contains("audit/freeform_freeze.rs"),
        "守卫自身是测试专用文件（由 audit.rs 的 #[cfg(test)] mod freeform_freeze; 声明而来）"
    );
    assert!(
        test_only.contains("db/tests.rs"),
        "db/tests.rs 由 db/mod.rs 的 #[cfg(test)] mod tests; 声明而来"
    );
    assert!(
        !test_only.contains("bot/registry.rs"),
        "bot/registry.rs 是生产文件（名字像测试文件不代表是）"
    );
}
