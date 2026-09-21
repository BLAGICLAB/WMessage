//! R6 observe CLI —— 跑一轮观察指标 / 检查停止条件
//!
//! 用法：
//!   cargo run --bin observe-run -- [options]
//!
//! 选项：
//!   --proposals <PATH>     evolution-proposals.jsonl 路径（默认：../evolution/evolution-proposals.jsonl）
//!   --changes <PATH>       evolution-changes.jsonl 路径
//!   --applied <PATH>       evolution-applied.jsonl 路径
//!   --window-days <N>      观察窗口天数（默认：30）
//!   --synthetic            用合成数据替换真实 jsonl（B 阶段验证用）
//!   --seed <N>             合成数据随机种子（默认：42）
//!   --output <PATH>        输出报告路径（默认：./observe-report.json）
//!   --check-stop           检查 R6 A 停止条件（要 R6 A 验收 D 项）
//!   --start-ms <EPOCH_MS>  R6 A flag 开启时刻（--check-stop 必填）

use std::path::PathBuf;
use wmessage_lib::eval::metrics;
use wmessage_lib::evolution::candidate;
use wmessage_lib::evolution::change;
use wmessage_lib::evolution::observe::{
    check_stop_condition, compute_metrics, generate_synthetic, write_to_files, SyntheticConfig,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut use_synthetic = false;
    let mut seed: u64 = 42;
    let mut window_days: i64 = 30;
    let mut check_stop = false;
    let mut start_ms: Option<i64> = None;
    let mut proposals_path = PathBuf::from("../evolution/evolution-proposals.jsonl");
    let mut changes_path = PathBuf::from("../evolution/evolution-changes.jsonl");
    let mut applied_path = PathBuf::from("../evolution/evolution-applied.jsonl");
    let mut output_path = PathBuf::from("./observe-report.json");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--proposals" => {
                proposals_path = PathBuf::from(args.get(i + 1).expect("--proposals 后需路径"));
                i += 2;
            }
            "--changes" => {
                changes_path = PathBuf::from(args.get(i + 1).expect("--changes 后需路径"));
                i += 2;
            }
            "--applied" => {
                applied_path = PathBuf::from(args.get(i + 1).expect("--applied 后需路径"));
                i += 2;
            }
            "--window-days" => {
                window_days = args
                    .get(i + 1)
                    .expect("--window-days 后需 N")
                    .parse()
                    .expect("N 必须是整数");
                i += 2;
            }
            "--synthetic" => {
                use_synthetic = true;
                i += 1;
            }
            "--seed" => {
                seed = args
                    .get(i + 1)
                    .expect("--seed 后需 N")
                    .parse()
                    .expect("N 必须是整数");
                i += 2;
            }
            "--output" => {
                output_path = PathBuf::from(args.get(i + 1).expect("--output 后需路径"));
                i += 2;
            }
            "--check-stop" => {
                check_stop = true;
                i += 1;
            }
            "--start-ms" => {
                start_ms = Some(
                    args.get(i + 1)
                        .expect("--start-ms 后需 EPOCH_MS")
                        .parse()
                        .expect("必须是整数"),
                );
                i += 2;
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

    // 老板 21:38 拍板：synthetic 隔离 — --synthetic 写 evolution.synthetic/，默认读 evolution/（真数据路径）
    if use_synthetic {
        let synth_base = PathBuf::from("../evolution.synthetic");
        proposals_path = synth_base.join("evolution-proposals.jsonl");
        changes_path = synth_base.join("evolution-changes.jsonl");
        applied_path = synth_base.join("evolution-applied.jsonl");
        output_path = PathBuf::from("./observe-report.synthetic.json");
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--proposals" => {
                proposals_path = PathBuf::from(args.get(i + 1).expect("--proposals 后需路径"));
                i += 2;
            }
            "--changes" => {
                changes_path = PathBuf::from(args.get(i + 1).expect("--changes 后需路径"));
                i += 2;
            }
            "--applied" => {
                applied_path = PathBuf::from(args.get(i + 1).expect("--applied 后需路径"));
                i += 2;
            }
            "--window-days" => {
                window_days = args
                    .get(i + 1)
                    .expect("--window-days 后需 N")
                    .parse()
                    .expect("N 必须是整数");
                i += 2;
            }
            "--synthetic" => {
                use_synthetic = true;
                i += 1;
            }
            "--seed" => {
                seed = args
                    .get(i + 1)
                    .expect("--seed 后需 N")
                    .parse()
                    .expect("N 必须是整数");
                i += 2;
            }
            "--output" => {
                output_path = PathBuf::from(args.get(i + 1).expect("--output 后需路径"));
                i += 2;
            }
            "--check-stop" => {
                check_stop = true;
                i += 1;
            }
            "--start-ms" => {
                start_ms = Some(
                    args.get(i + 1)
                        .expect("--start-ms 后需 EPOCH_MS")
                        .parse()
                        .expect("必须是整数"),
                );
                i += 2;
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

    let now_ms = chrono::Utc::now().timestamp_millis();

    // ─── 模式：check-stop（R6 A 验收 D 项）───
    if check_stop {
        let start = start_ms.expect("--check-stop 必须配 --start-ms");
        let changes = change::read_all(&changes_path).expect("读 evolution-changes 失败");
        let status = check_stop_condition(&changes, start, now_ms);
        println!("=== R6 A 停止条件检查 ===");
        println!(
            "changes_completed: {}/{}  {}",
            status.changes_completed,
            30,
            if status.changes_completed >= 30 {
                "✓ 触发"
            } else {
                "✗"
            }
        );
        println!(
            "days_elapsed:     {:.1}/14.0  {}",
            status.days_elapsed,
            if status.days_elapsed >= 14.0 {
                "✓ 触发"
            } else {
                "✗"
            }
        );
        println!(
            "rolled_back:      {}/{}  {}",
            status.rolled_back_count,
            5,
            if status.rolled_back_count >= 5 {
                "✓ 触发"
            } else {
                "✗"
            }
        );
        println!("----");
        if status.should_stop {
            println!("should_stop: true  ←  满足任一停止条件，请关闭 evolution.shadow.enabled");
            for r in &status.stop_reasons {
                println!("  reason: {}", r.as_str());
            }
            std::process::exit(0);
        } else {
            println!("should_stop: false  （未达停止条件，继续收集）");
        }
        return;
    }

    // ─── 默认模式：metrics ───
    let window_start_ms = now_ms - window_days * 86_400_000;

    let proposals: Vec<_>;
    let changes: Vec<_>;
    let applied: Vec<_>;

    if use_synthetic {
        eprintln!(
            "[observe-run] 用合成数据（seed={}，proposal_total=100）",
            seed
        );
        let cfg = SyntheticConfig {
            seed,
            ..SyntheticConfig::default()
        };
        let data = generate_synthetic(&cfg, now_ms);
        write_to_files(&data, &proposals_path, &changes_path, &applied_path)
            .expect("写合成数据失败");
        proposals = data.proposals;
        changes = data.changes;
        applied = data.applied;
    } else {
        proposals = candidate::read_all(&proposals_path).expect("读 evolution-proposals 失败");
        changes = change::read_all(&changes_path).expect("读 evolution-changes 失败");
        applied = metrics::read_applied(&applied_path).expect("读 evolution-applied 失败");
    }

    let report = compute_metrics(&proposals, &changes, &applied, now_ms, window_start_ms);

    let json = serde_json::to_string_pretty(&report).expect("序列化 MetricsReport");
    println!("{json}");

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("建目录失败");
    }
    std::fs::write(&output_path, &json).expect("写观察报告失败");
    eprintln!("[observe-run] 报告写入：{output_path:?}");
}

fn print_help() {
    println!(
        "observe-run — 跑一轮 R6 观察指标 / 检查停止条件

用法:
  cargo run --bin observe-run -- [options]

参数:
  --proposals PATH    evolution-proposals.jsonl 路径 (默认: ../evolution/evolution-proposals.jsonl)
  --changes PATH      evolution-changes.jsonl 路径
  --applied PATH      evolution-applied.jsonl 路径
  --window-days N     观察窗口天数 (默认: 30)
  --synthetic         用合成数据替换真实 jsonl（B 阶段验证用）
  --seed N            合成数据随机种子 (默认: 42)
  --output PATH       输出报告路径 (默认: ./observe-report.json)
  --check-stop        检查 R6 A 停止条件（要配 --start-ms）
  --start-ms N        R6 A flag 开启时刻 (epoch ms)，--check-stop 必填
"
    );
}
