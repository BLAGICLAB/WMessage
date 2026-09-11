//! 嵌入引擎（memory v2，设计 docs/BOT-MEMORY-V2-DESIGN.md）：
//! bge-small-zh-v1.5（ONNX 量化版，hidden=512）本地推理。
//!
//! 设计要点：
//! - 全局懒加载（OnceCell）：首次 embed 时才加载 tokenizer + ONNX session；
//!   加载失败把原因缓存下来并永久返回 None —— 全系统优雅降级为纯关键词模式，
//!   绝不让聊天/写入路径因模型缺失而失败。
//! - 推理必须在阻塞线程跑：本模块的公开入口（embed_text）是同步的，
//!   调用方（memory/mod.rs 门面）一律在 spawn_blocking 闭包里调它，
//!   不在 async 运行时 worker 上直接跑 ONNX 推理。
//! - 首次成功加载后后台线程跑一次 dummy 推理预热（JIT/内存页准备，
//!   避免首轮真实请求付冷启动延迟）。

use std::sync::Mutex;

/// 嵌入维度（bge-small-zh-v1.5 hidden_size）
pub const EMBED_DIM: usize = 512;

/// 输入截断上限（模型 max_position_embeddings）
const MAX_TOKENS: usize = 512;

/// 模型目录名（打包后放 exe 同目录；开发时在仓库根）
const MODEL_DIR_NAME: &str = "bge-small-zh-v1.5";

/// 环境变量覆盖（优先级最高）：指向模型目录（内含 onnx/model_quantized.onnx 与 tokenizer.json）
pub const MODEL_DIR_ENV: &str = "WMESSAGE_BGE_MODEL_DIR";

struct Engine {
    tokenizer: tokenizers::Tokenizer,
    // ort Session 推理非线程安全语义由 Mutex 保证（记忆体量小，串行足够）
    session: Mutex<ort::session::Session>,
}

/// 加载结果缓存：Ok=引擎可用；Err=降级原因（只记一次，供审计/排查）
static ENGINE: std::sync::OnceLock<Result<Engine, String>> = std::sync::OnceLock::new();

/// 模型目录解析（纯函数，便于单测）：
/// 优先级 = 显式参数（环境变量值）→ exe 同目录/MODEL_DIR_NAME → 开发仓库根
/// （CARGO_MANIFEST_DIR/../MODEL_DIR_NAME）。返回第一个「必要文件齐全」的目录。
fn pick_model_dir(
    env_dir: Option<&str>,
    exe_dir: Option<std::path::PathBuf>,
    dev_dir: std::path::PathBuf,
) -> Option<std::path::PathBuf> {
    fn usable(dir: &std::path::Path) -> bool {
        dir.join("onnx/model_quantized.onnx").is_file() && dir.join("tokenizer.json").is_file()
    }
    if let Some(d) = env_dir.map(str::trim).filter(|d| !d.is_empty()) {
        let p = std::path::PathBuf::from(d);
        // 显式指定时不回退：路径写错应当显式降级并在日志里指出，而不是静默换目录
        return usable(&p).then_some(p);
    }
    if let Some(exe) = exe_dir {
        let p = exe.join(MODEL_DIR_NAME);
        if usable(&p) {
            return Some(p);
        }
    }
    let dev = dev_dir;
    usable(&dev).then_some(dev)
}

/// 生产路径解析：env → exe 同目录 → 仓库根
fn resolve_model_dir() -> Option<std::path::PathBuf> {
    let env_dir = std::env::var(MODEL_DIR_ENV).ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let dev_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join(MODEL_DIR_NAME))
        .unwrap_or_default();
    pick_model_dir(env_dir.as_deref(), exe_dir, dev_dir)
}

fn load_engine() -> Result<Engine, String> {
    let dir = resolve_model_dir()
        .ok_or_else(|| format!("模型目录缺失（已检查 {MODEL_DIR_ENV} / exe 同目录 / 仓库根）"))?;
    let mut tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
        .map_err(|e| format!("tokenizer.json 加载失败：{e}"))?;
    tokenizer
        .with_truncation(Some(tokenizers::TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }))
        .map_err(|e| format!("tokenizer 截断配置失败：{e}"))?;
    let session = ort::session::Session::builder()
        .map_err(|e| format!("onnxruntime 初始化失败：{e}"))?
        .with_intra_threads(2)
        .map_err(|e| format!("onnxruntime 线程配置失败：{e}"))?
        .commit_from_file(dir.join("onnx/model_quantized.onnx"))
        .map_err(|e| format!("ONNX 模型加载失败：{e}"))?;
    Ok(Engine {
        tokenizer,
        session: Mutex::new(session),
    })
}

/// 引擎句柄（触发懒加载）；失败原因进程内只算一次
fn engine() -> Result<&'static Engine, &'static str> {
    ENGINE
        .get_or_init(|| {
            let r = load_engine();
            match &r {
                Ok(_) => eprintln!("[memory] 嵌入引擎加载成功"),
                Err(e) => eprintln!("[memory] 嵌入引擎不可用，降级为关键词模式：{e}"),
            }
            r
        })
        .as_ref()
        .map_err(|e| e.as_str())
}

/// attention-mask 加权 mean pooling（纯函数，单测可绕开 ONNX 直测）：
/// 对 last_hidden_state [seq, hidden] 按 mask（1=有效 token）加权平均。
pub(crate) fn mean_pool(
    hidden_states: &[f32],
    seq_len: usize,
    hidden: usize,
    mask: &[f32],
) -> Vec<f32> {
    let mut out = vec![0f32; hidden];
    let mut denom = 0f32;
    for t in 0..seq_len {
        let m = mask.get(t).copied().unwrap_or(0.0);
        if m <= 0.0 {
            continue;
        }
        denom += m;
        let row = &hidden_states[t * hidden..(t + 1) * hidden];
        for (o, v) in out.iter_mut().zip(row) {
            *o += v * m;
        }
    }
    if denom > 0.0 {
        for o in out.iter_mut() {
            *o /= denom;
        }
    }
    out
}

/// L2 归一化（纯函数）；零向量原样返回（不 NaN）
pub(crate) fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// 文本 → 512 维 L2 归一化向量。任何一步失败（含引擎未加载）都返回 None ——
/// 调用方按「无向量」降级处理（语义项权重归一给关键词/重要度/新近度）。
/// 同步阻塞接口：只能在阻塞线程调用（门面层保证）。
pub fn embed_text(text: &str) -> Option<Vec<f32>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let eng = engine().ok()?;
    let enc = eng.tokenizer.encode(text, true).ok()?;
    let seq = enc.get_ids().len();
    if seq == 0 {
        return None;
    }
    let input_ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
    let attn: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
    let types: Vec<i64> = enc.get_type_ids().iter().map(|&x| x as i64).collect();
    let ids_t = ort::value::Tensor::from_array(([1usize, seq], input_ids)).ok()?;
    let attn_t = ort::value::Tensor::from_array(([1usize, seq], attn.clone())).ok()?;
    let types_t = ort::value::Tensor::from_array(([1usize, seq], types)).ok()?;
    let mut session = eng.session.lock().ok()?;
    let outputs = session.run(ort::inputs![ids_t, attn_t, types_t]).ok()?;
    let mut iter = outputs.iter();
    let Some((_, value)) = iter.next() else {
        return None;
    };
    // 取第一个输出（bge 导出版只有 last_hidden_state [1, seq, 512]）
    let Ok((_shape, data)) = value.try_extract_tensor::<f32>() else {
        return None;
    };
    if data.len() != seq * EMBED_DIM {
        return None;
    }
    let mask_f: Vec<f32> = attn.iter().map(|&x| x as f32).collect();
    let mut emb = mean_pool(data, seq, EMBED_DIM, &mask_f);
    l2_normalize(&mut emb);
    Some(emb)
}

/// 首次成功加载后后台预热一次 dummy 推理（冷热路径分离，不阻塞首个真实请求方）。
/// 幂等：OnceLock 保证只触发一次；引擎不可用时什么都不做。
pub fn warmup_async() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        if engine().is_ok() {
            let _ = embed_text("预热");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_pool_weights_by_attention_mask() {
        // seq=3, hidden=2；mask=[1,0,1] → 只平均第 0/2 行
        let hs = [1.0, 2.0, 100.0, 100.0, 3.0, 4.0];
        let out = mean_pool(&hs, 3, 2, &[1.0, 0.0, 1.0]);
        assert_eq!(out, vec![2.0, 3.0]);
    }

    #[test]
    fn mean_pool_all_masked_returns_zeros() {
        let hs = [1.0, 2.0, 3.0, 4.0];
        let out = mean_pool(&hs, 2, 2, &[0.0, 0.0]);
        assert_eq!(out, vec![0.0, 0.0]);
    }

    #[test]
    fn l2_normalize_unit_norm() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize(&mut v);
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((v[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_vector_no_nan() {
        let mut v = vec![0.0f32; 4];
        l2_normalize(&mut v);
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn pick_model_dir_prefers_explicit_env() {
        let usable_env =
            std::env::temp_dir().join(format!("wm_bge_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(usable_env.join("onnx")).unwrap();
        std::fs::write(usable_env.join("onnx/model_quantized.onnx"), b"x").unwrap();
        std::fs::write(usable_env.join("tokenizer.json"), b"{}").unwrap();
        let r = pick_model_dir(
            Some(usable_env.to_str().unwrap()),
            Some(std::path::PathBuf::from("/nonexistent-exe-dir")),
            std::path::PathBuf::from("/nonexistent-dev"),
        );
        assert_eq!(r, Some(usable_env.clone()));
        std::fs::remove_dir_all(&usable_env).ok();
    }

    #[test]
    fn pick_model_dir_missing_everywhere_returns_none() {
        let r = pick_model_dir(
            Some("/nonexistent-explicit-dir"),
            Some(std::path::PathBuf::from("/nonexistent-exe-dir")),
            std::path::PathBuf::from("/nonexistent-dev"),
        );
        assert_eq!(r, None, "显式目录不可用时不回退（显式降级）");
        let r2 = pick_model_dir(
            None,
            Some(std::path::PathBuf::from("/nonexistent-exe-dir")),
            std::path::PathBuf::from("/nonexistent-dev"),
        );
        assert_eq!(r2, None);
    }

    /// 真实 ONNX 模型冒烟（需要仓库根 bge-small-zh-v1.5/ 在场）：
    /// 维度 512、L2 归一、语义近义句余弦显著高于无关句。
    /// 手动跑：cargo test --lib memory::embed -- --ignored
    #[test]
    #[ignore = "真实模型推理冒烟（慢，手动跑）"]
    fn real_model_embed_smoke() {
        let a = embed_text("我不吃辣").expect("模型应可用（仓库根应有 bge-small-zh-v1.5）");
        let b = embed_text("我讨厌辛辣的食物").expect("embed");
        let c = embed_text("明天下午三点开会").expect("embed");
        assert_eq!(a.len(), EMBED_DIM);
        let norm = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "应 L2 归一化：{norm}");
        let sim_ab = crate::memory::store::cosine(Some(&a), Some(&b)).unwrap();
        let sim_ac = crate::memory::store::cosine(Some(&a), Some(&c)).unwrap();
        assert!(sim_ab > sim_ac, "近义句相似度应更高：{sim_ab} vs {sim_ac}");
        assert!(sim_ab > 0.5, "近义句余弦过低：{sim_ab}");
    }
}
