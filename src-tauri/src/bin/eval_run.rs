//! CLI：跑一轮 eval，输出指标
//!
//! 用法：
//!   cargo run --bin eval-run -- [options]
//!
//! 选项：
//!   --config <PATH>     bot-config.json 路径（默认：./bot-config.json）
//!   --db <PATH>         dev DB 路径（可选；不传则跳过 mem_items 状态读取）
//!   --period <LABEL>    周期标签（默认：manual）
//!   --out <PATH>        evolution-eval-results.jsonl 输出路径
//!                       （默认：底稿所在目录 /evolution-eval-results.jsonl）
//!   --quiet             只打印指标 JSON，不打 verbose 日志

use std::path::PathBuf;
use wmessage_lib::eval;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = PathBuf::from("bot-config.json");
    let mut db_path: Option<PathBuf> = None;
    let mut period = "manual".to_string();
    let mut out_path: Option<PathBuf> = None;
    let mut quiet = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                config_path = PathBuf::from(args.get(i + 1).expect("--config 后需路径"));
                i += 2;
            }
            "--db" => {
                db_path = Some(PathBuf::from(args.get(i + 1).expect("--db 后需路径")));
                i += 2;
            }
            "--period" => {
                period = args.get(i + 1).expect("--period 后需标签").clone();
                i += 2;
            }
            "--out" => {
                out_path = Some(PathBuf::from(args.get(i + 1).expect("--out 后需路径")));
                i += 2;
            }
            "--quiet" => {
                quiet = true;
                i += 1;
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }
    let cfg = match eval::load_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("加载 bot-config.json 失败：{e}");
            std::process::exit(1);
        }
    };
    if !quiet {
        eprintln!("[eval-run] eval_set: {:?}", cfg.eval_set_path);
        eprintln!("[eval-run] feedback: {:?}", cfg.feedback_path);
        eprintln!("[eval-run] period: {period}");
    }
    let report = match eval::run_eval(&cfg, db_path.as_deref(), &period) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("跑评估失败：{e}");
            std::process::exit(1);
        }
    };
    // 输出：stdout JSON
    let json = serde_json::to_string_pretty(&report).expect("序列化 MetricsReport");
    println!("{json}");
    // 追加到结果 jsonl
    let out = out_path.unwrap_or_else(|| {
        // 默认：eval_set 同目录的 evolution-eval-results.jsonl
        let mut p = cfg.eval_set_path.clone();
        p.set_file_name("evolution-eval-results.jsonl");
        p
    });
    if let Err(e) = eval::append_result(&out, &report) {
        eprintln!("写结果 {out:?} 失败：{e}");
        std::process::exit(1);
    }
    if !quiet {
        eprintln!("[eval-run] 写入：{out:?}");
    }
}

fn print_help() {
    println!(
        "eval-run — 跑一轮 eval，输出五个指标

用法:
  cargo run --bin eval-run -- [--config PATH] [--db PATH] [--period LABEL] [--out PATH] [--quiet]

参数:
  --config PATH    bot-config.json 路径 (默认: ./bot-config.json)
  --db PATH        dev DB 路径 (可选)
  --period LABEL   周期标签 (默认: manual)
  --out PATH       结果 jsonl 路径 (默认: eval_set 同目录的 evolution-eval-results.jsonl)
  --quiet          静默模式（只打印指标 JSON）
"
    );
}
