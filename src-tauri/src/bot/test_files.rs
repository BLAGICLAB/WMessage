//! 源码结构识别：哪些文件是「测试专用文件」。
//!
//! 存在的理由：仓库里有两个门禁要扫 `src/` 全树找生产代码里的坏味道
//! （`audit.rs` 的 free-form 审计入口冻结、`bot/procs` 的控制台程序静默标志）。
//! 它们都必须先把「测试代码」剔除，否则测试代码里的违规会被当成生产违规。
//!
//! 判据是**模块声明**而不是文件名或代码位置：
//!
//! - 父文件里 `#[cfg(test)] mod X;` 声明出来的子模块文件，整个文件只
//!   在测试构建里存在 → 整份都是测试代码。
//! - Rust 的模块文件规则是唯一的：父文件 `src/a.rs` 与 `src/a/mod.rs`
//!   的子模块都在 `src/a/X.rs`——所以从声明就能算出子文件路径。
//!
//! 为什么不按文件名猜：`bot/mcp/manager/e2e_stdio.rs` 与
//! `bot/eval.rs` 是测试专用但名字里没有 "test"；`bot/registry.rs` 名字
//! 像测试文件却是生产代码。猜名字两处都错。
//!
//! 为什么不按代码位置猜（「首个 `#[cfg(test)]` 之前的部分」）：那假设测试
//! 代码都在文件末尾。`bot_fs.rs` 的首个 `#[cfg(test)]` 在第 75 行，于是它
//! 2000 多行生产代码整段落在「测试区」里，守卫根本扫不到——扫描范围取决于
//! 代码摆放位置，而不是代码是不是测试。

// 本模块只服务测试里的源码扫描守卫，生产构建用不到它。
#![cfg(test)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 收集 `src/` 下所有测试专用文件的相对路径（相对 `src/`，`/` 分隔）。
pub(crate) fn test_only_files(src: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
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
                if let Some(child) = child_module_path(&p, &name) {
                    if let Ok(rel) = child.strip_prefix(src) {
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
/// 同一个文件里，由调用方自己的「剔除内联块」逻辑处理。
pub(crate) fn cfg_test_mod_decls(body: &str) -> Vec<String> {
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

/// 父文件声明的子模块 `X` 对应的文件路径。
///
/// Rust 的模块文件规则：父文件 `src/a.rs` 的子模块在 `src/a/X.rs`；
/// 父文件 `src/a/mod.rs` 的子模块也在 `src/a/X.rs`。
fn child_module_path(parent: &Path, name: &str) -> Option<PathBuf> {
    let file = format!("{name}.rs");
    if parent.file_name().is_some_and(|n| n == "mod.rs") {
        parent.parent().map(|d| d.join(file))
    } else {
        let stem = parent.file_stem()?;
        parent.parent().map(|d| d.join(stem).join(file))
    }
}

/// 一个文件里的「生产切片」：剔除全部 `#[cfg(test)]` 块。
///
/// 与文件布局无关：生产代码无论在文件哪个位置都留在结果里，测试代码无论
/// 内联还是独立成文件都不进结果（独立成文件的由 `test_only_files` 负责）。
pub(crate) fn prod_slice(body: &str) -> String {
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
