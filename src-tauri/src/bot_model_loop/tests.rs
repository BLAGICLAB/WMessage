//! 模型循环的单元测试：按关注点分三桶（parse / guard / config）。
//!
//! 这些 mod 原先内联在 bot_model_loop.rs 尾部，拆分时逐字搬移。
//!
//! 为什么单独成文件：只被测试用的项不该从门面转出——门面 `pub(crate) use`
//! 一个仅测试引用的项，在非测试构建里会被 rustc 判为 unused import 而报错。
//! 这类项由真正用它的桶文件直接从子模块取。

use crate::bot_model_loop::*;

/// 生产代码全文（门面 + 全部子模块），供源码守卫扫。
///
/// 拆分前源码守卫扫的是 bot_model_loop.rs 单文件；拆分后代码分布在一批文件里，
/// 守卫必须扫全景，否则「生产调用被删掉」这类变异在别的文件里发生时就扫不到。
///
/// 注意这里只列生产文件：把测试文件也拼进来会让守卫自匹配——守卫要找的字符串
/// 字面量就在测试文件里，生产调用真被删掉后 find 仍会命中它，测试照绿，
/// 那才是真的假保证。
fn prod_src() -> String {
    [include_str!("../bot_model_loop.rs")].concat()
}

/// 含指定调用点的那个生产文件的全文（运行时从磁盘读）。
///
/// 为什么不能拿 prod_src() 的拼接结果代替：有的守卫要的是「调用点之前紧邻
/// stop 检查、两者之间没有别的 await」——这种**邻接关系只在同一个文件里成立**。
/// 把多个文件拼起来扫，别个文件的 stop 检查会混进前缀里，邻接断言就失去意义
/// （守卫变松，而不是变严）。
///
/// 所以这里按调用点定位文件，再只扫那一个文件。
fn prod_file_with(needle: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("bot_model_loop.rs")];
    let dir = root.join("bot_model_loop");
    for e in std::fs::read_dir(&dir).expect("应能读 bot_model_loop 目录") {
        let p = e.expect("目录项").path();
        // tests.rs 是测试自留地，不算生产代码
        if p.extension().is_some_and(|x| x == "rs")
            && p.file_name().is_some_and(|n| n != "tests.rs")
        {
            files.push(p);
        }
    }
    for p in files {
        let body = std::fs::read_to_string(&p).expect("应能读生产文件");
        if body.contains(needle) {
            return body;
        }
    }
    panic!("应能在 bot_model_loop 族里找到调用点 {needle}");
}

mod config;
mod guard;
mod parse;
