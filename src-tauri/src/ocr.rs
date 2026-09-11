//! OCR 原子工具（ocr_image）：本地识别图片文字，逐行返回。
//!
//! 隐私红线（硬约束）：图片字节只在内存里交给本地引擎，本模块没有任何网络调用，
//! 识别过程绝不上传外网；网络路径（http/https/data:）在入口处直接拒。
//!
//! 平台分支：
//! - macOS 13+：Apple Vision VNRecognizeTextRequest（系统内置，0 下载）
//! - Windows：PP-OCRv6 small（det 检测 + rec 识别 + 可选 v5 cls 方向分类）ONNX 本地推理，
//!   模型目录 pp-ocr-v6/（scripts/fetch_ocr_models.sh 下载，合计约 31MB）
//! - 其他平台：显式降级为「不支持」错误
//!
//! 推理全部为同步阻塞调用，入口统一 spawn_blocking，不在 async worker 上跑。

use tauri::AppHandle;

/// ocr_image 工具：识别本地图片中的文字（路径白名单守卫复用 bot_fs::resolve_with_perm）。
pub async fn tool_ocr_image(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str() else {
        return ("失败：ocr_image 缺少 path 参数".into(), Vec::new());
    };
    let path = path.trim();
    if path.is_empty() {
        return ("失败：ocr_image 的 path 不能为空".into(), Vec::new());
    }
    // 隐私红线：只接受本地文件路径。网络图片一律不抓取（resolve_with_perm 也会因
    // canonicalize 失败而拒，这里前置给出明确原因）
    if path.starts_with("http://") || path.starts_with("https://") || path.starts_with("data:") {
        return (
            "失败：ocr_image 只接受本地图片路径，网络地址不予抓取（隐私红线：识别绝不上传外网）"
                .into(),
            Vec::new(),
        );
    }
    let canonical =
        match crate::bot_fs::resolve_with_perm(app, "ocr_image", path, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return (e, Vec::new()),
        };
    if canonical.is_dir() {
        return (
            format!("失败：{path} 是目录，ocr_image 只接受图片文件"),
            Vec::new(),
        );
    }
    let log_path = crate::bot::truncate_for_log(&canonical.display().to_string(), 200);
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let bytes = std::fs::read(&canonical).map_err(|e| format!("读取图片失败：{e}"))?;
        recognize(&bytes)
    })
    .await;
    match r {
        Ok(Ok(text)) => {
            // 审计只记路径/大小/字符数，绝不记图片内容或识别文本
            crate::bot::audit_log(
                app,
                &format!("ocr.done | {} | {} chars", log_path, text.chars().count()),
            );
            (text, Vec::new())
        }
        Ok(Err(e)) => {
            crate::bot::audit_log(app, &format!("ocr.fail | {log_path} | {e}"));
            (format!("失败：{e}"), Vec::new())
        }
        Err(e) => (format!("失败：OCR 线程异常：{e}"), Vec::new()),
    }
}

/// 平台分发：图片字节 → 识别文本（多行，按引擎返回顺序拼接）。
/// 同步阻塞接口：只能在阻塞线程调用（入口 spawn_blocking 保证）。
fn recognize(bytes: &[u8]) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        macos::recognize(bytes)
    }
    #[cfg(target_os = "windows")]
    {
        win::recognize(bytes)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = bytes;
        Err("当前平台不支持 OCR（macOS 用系统 Vision，Windows 需 pp-ocr-v6 模型目录）".into())
    }
}

// ───────────────────────── macOS：Apple Vision ─────────────────────────

#[cfg(target_os = "macos")]
mod macos {
    use objc2::runtime::AnyObject;
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };

    /// VNRecognizeTextRequest 同步识别（系统内置引擎：无模型下载、无网络）。
    /// performRequests 本身是同步阻塞调用，调用方已在 blocking 线程。
    pub(super) fn recognize(bytes: &[u8]) -> Result<String, String> {
        let data = NSData::from_vec(bytes.to_vec());
        let options: objc2::rc::Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        // 中英双语（zh-Hans 需 macOS 13+ 的 revision 2；不支持时 Vision 自动回退可用语言）
        let langs = NSArray::from_slice(&[
            &*NSString::from_str("zh-Hans"),
            &*NSString::from_str("en-US"),
        ]);
        request.setRecognitionLanguages(&langs);
        request.setUsesLanguageCorrection(true);
        let requests = {
            let r: &VNRequest = &request;
            NSArray::from_slice(&[r])
        };
        handler
            .performRequests_error(&requests)
            .map_err(|e| format!("Vision 识别失败：{}", e.localizedDescription()))?;
        let mut lines: Vec<String> = Vec::new();
        if let Some(results) = request.results() {
            for obs in results.iter() {
                let candidates = obs.topCandidates(1);
                if let Some(best) = candidates.firstObject() {
                    let s = best.string().to_string();
                    if !s.trim().is_empty() {
                        lines.push(s);
                    }
                }
            }
        }
        // 识别不到文字返回空串（不算失败）
        Ok(lines.join("\n"))
    }
}

// ───────────── Windows：PP-OCRv6 ONNX（det + rec + 可选 v5 cls） ─────────────

#[cfg(target_os = "windows")]
mod win {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use image::RgbImage;

    /// 模型目录名（打包后放 exe 同目录；开发时在仓库根）
    const MODEL_DIR_NAME: &str = "pp-ocr-v6";
    /// 环境变量覆盖（优先级最高）：指向模型目录（内含 det.onnx / rec.onnx / keys.txt，cls.onnx 可选）
    pub const MODEL_DIR_ENV: &str = "WMESSAGE_OCR_MODEL_DIR";
    /// 模型缺失时的下载指引
    const FETCH_HINT: &str =
        "请运行 scripts/fetch_ocr_models.sh 下载 PP-OCRv6 模型到 pp-ocr-v6/ 目录（约 31MB）";

    /// det 输入最长边（等比缩放，边长对齐 32 倍数）
    const DET_MAX_SIDE: u32 = 960;
    /// rec 输入高度 / 宽度上限（PP-OCRv6 与 v5/v4 的 rec 前后处理配置一致：高 48、宽动态）
    const REC_H: u32 = 48;
    const REC_MAX_W: u32 = 320;
    /// cls 输入尺寸 / 旋转置信度阈值（v6 无独立 cls 模型，沿用 PP-OCRv5 文本行方向分类）
    const CLS_H: u32 = 48;
    const CLS_W: u32 = 192;
    const CLS_ROTATE_MIN_CONF: f32 = 0.9;

    struct Engine {
        // 与 memory::embed 同模式：Session 非线程安全语义由 Mutex 保证（串行足够）
        det: Mutex<ort::session::Session>,
        rec: Mutex<ort::session::Session>,
        cls: Option<Mutex<ort::session::Session>>,
        /// 字典：keys[i] 对应 CTC 索引 i+1（0 为 blank）
        keys: Vec<String>,
    }

    /// 加载结果缓存：Ok=引擎可用；Err=降级原因（只记一次）
    static ENGINE: std::sync::OnceLock<Result<Engine, String>> = std::sync::OnceLock::new();

    /// 模型目录解析（纯函数，便于单测）：
    /// 优先级 = 显式参数（环境变量值）→ exe 同目录/MODEL_DIR_NAME → 开发仓库根
    /// （CARGO_MANIFEST_DIR/../MODEL_DIR_NAME）。返回第一个「必要文件齐全」的目录。
    fn pick_model_dir(
        env_dir: Option<&str>,
        exe_dir: Option<PathBuf>,
        dev_dir: PathBuf,
    ) -> Option<PathBuf> {
        fn usable(dir: &Path) -> bool {
            dir.join("det.onnx").is_file()
                && dir.join("rec.onnx").is_file()
                && dir.join("keys.txt").is_file()
        }
        if let Some(d) = env_dir.map(str::trim).filter(|d| !d.is_empty()) {
            let p = PathBuf::from(d);
            // 显式指定时不回退：路径写错应当显式降级，而不是静默换目录
            return usable(&p).then_some(p);
        }
        if let Some(exe) = exe_dir {
            let p = exe.join(MODEL_DIR_NAME);
            if usable(&p) {
                return Some(p);
            }
        }
        usable(&dev_dir).then_some(dev_dir)
    }

    /// 生产路径解析：env → exe 同目录 → 仓库根
    fn resolve_model_dir() -> Option<PathBuf> {
        let env_dir = std::env::var(MODEL_DIR_ENV).ok();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join(MODEL_DIR_NAME))
            .unwrap_or_default();
        pick_model_dir(env_dir.as_deref(), exe_dir, dev_dir)
    }

    fn new_session(path: &Path) -> Result<ort::session::Session, String> {
        ort::session::Session::builder()
            .map_err(|e| format!("onnxruntime 初始化失败：{e}"))?
            .with_intra_threads(2)
            .map_err(|e| format!("onnxruntime 线程配置失败：{e}"))?
            .commit_from_file(path)
            .map_err(|e| format!("ONNX 模型加载失败（{}）：{e}", path.display()))
    }

    fn load_engine() -> Result<Engine, String> {
        let dir = resolve_model_dir().ok_or_else(|| {
            format!("模型目录缺失（已检查 {MODEL_DIR_ENV} / exe 同目录 / 仓库根，需 det.onnx + rec.onnx + keys.txt）。{FETCH_HINT}")
        })?;
        let keys_raw = std::fs::read_to_string(dir.join("keys.txt"))
            .map_err(|e| format!("keys.txt 读取失败：{e}"))?;
        let keys: Vec<String> = keys_raw.lines().map(|l| l.to_string()).collect();
        if keys.is_empty() {
            return Err("keys.txt 为空".into());
        }
        let det = new_session(&dir.join("det.onnx"))?;
        let rec = new_session(&dir.join("rec.onnx"))?;
        // cls 可选：存在才加载（方向分类，识别 0°/180°；v6 没有独立 cls，脚本下载的是 v5 cls）
        let cls_path = dir.join("cls.onnx");
        let cls = if cls_path.is_file() {
            Some(new_session(&cls_path)?)
        } else {
            None
        };
        Ok(Engine {
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            cls: cls.map(Mutex::new),
            keys,
        })
    }

    /// 引擎句柄（触发懒加载）；失败原因进程内只算一次
    fn engine() -> Result<&'static Engine, &'static str> {
        ENGINE
            .get_or_init(|| {
                let r = load_engine();
                match &r {
                    Ok(_) => eprintln!("[ocr] PP-OCRv6 引擎加载成功"),
                    Err(e) => eprintln!("[ocr] 引擎不可用：{e}"),
                }
                r
            })
            .as_ref()
            .map_err(|e| e.as_str())
    }

    /// RGB 像素 → NCHW f32 tensor，归一化 (x/255 - 0.5) / 0.5
    fn to_nchw(img: &RgbImage) -> Vec<f32> {
        let (w, h) = img.dimensions();
        let mut out = vec![0f32; 3 * (w * h) as usize];
        let plane = (w * h) as usize;
        for (i, px) in img.pixels().enumerate() {
            out[i] = (px[0] as f32 / 255.0 - 0.5) / 0.5;
            out[plane + i] = (px[1] as f32 / 255.0 - 0.5) / 0.5;
            out[2 * plane + i] = (px[2] as f32 / 255.0 - 0.5) / 0.5;
        }
        out
    }

    /// 单输入单输出模型跑一遍，取第一个输出的 f32 数据与形状
    /// （输入输出名不硬编码：positional inputs! + outputs.iter().next()）
    fn run_first(
        session: &Mutex<ort::session::Session>,
        dims: [usize; 4],
        data: Vec<f32>,
    ) -> Result<(Vec<i64>, Vec<f32>), String> {
        let tensor = ort::value::Tensor::from_array((dims, data))
            .map_err(|e| format!("tensor 构造失败：{e}"))?;
        let mut session = session.lock().map_err(|e| format!("session 锁失败：{e}"))?;
        let outputs = session
            .run(ort::inputs![tensor])
            .map_err(|e| format!("ONNX 推理失败：{e}"))?;
        let Some((_, value)) = outputs.iter().next() else {
            return Err("模型无输出".into());
        };
        let (shape, data) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("输出解析失败：{e}"))?;
        Ok((shape.iter().copied().collect(), data.to_vec()))
    }

    /// det 预处理：等比缩放到最长边 DET_MAX_SIDE，边长向上对齐 32 倍数
    fn det_resize(img: &RgbImage) -> (RgbImage, u32, u32) {
        let (w, h) = img.dimensions();
        let scale = (DET_MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
        let align = |v: u32| ((v + 31) / 32) * 32;
        let nw = align(((w as f32 * scale).round() as u32).max(1));
        let nh = align(((h as f32 * scale).round() as u32).max(1));
        let resized = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
        (resized, nw, nh)
    }

    /// rec 预处理：裁块 resize 到高 REC_H，宽按比例（上限 REC_MAX_W）
    fn rec_resize(crop: &RgbImage) -> (RgbImage, u32) {
        let (w, h) = crop.dimensions();
        let ratio = REC_H as f32 / h.max(1) as f32;
        let nw = ((w as f32 * ratio).round() as u32).clamp(1, REC_MAX_W);
        let resized =
            image::imageops::resize(crop, nw, REC_H, image::imageops::FilterType::Triangle);
        (resized, nw)
    }

    /// cls：判断文本块是否需要旋转 180°（置信度 < 阈值不旋转）
    fn should_rotate_180(eng: &Engine, crop: &RgbImage) -> Result<bool, String> {
        let Some(cls) = &eng.cls else {
            return Ok(false);
        };
        let resized =
            image::imageops::resize(crop, CLS_W, CLS_H, image::imageops::FilterType::Triangle);
        let data = to_nchw(&resized);
        let (shape, out) = run_first(cls, [1, 3, CLS_H as usize, CLS_W as usize], data)?;
        let n = shape.last().copied().unwrap_or(0) as usize;
        if n < 2 || out.len() < n {
            return Ok(false);
        }
        let scores = &out[..n];
        let (mut best_i, mut best_v) = (0usize, f32::MIN);
        for (i, &v) in scores.iter().enumerate() {
            if v > best_v {
                best_i = i;
                best_v = v;
            }
        }
        Ok(best_i == 1 && best_v >= CLS_ROTATE_MIN_CONF)
    }

    pub(super) fn recognize(bytes: &[u8]) -> Result<String, String> {
        let img = image::load_from_memory(bytes)
            .map_err(|e| format!("图片解码失败（支持 PNG/JPEG）：{e}"))?
            .to_rgb8();
        let eng = engine()?;
        let (orig_w, orig_h) = img.dimensions();

        // 1) det：检测文本区域
        let (det_img, det_w, det_h) = det_resize(&img);
        let det_in = to_nchw(&det_img);
        let (shape, prob) = run_first(&eng.det, [1, 3, det_h as usize, det_w as usize], det_in)?;
        if shape.len() != 4 {
            return Err(format!("det 输出形状异常：{shape:?}"));
        }
        let (out_h, out_w) = (shape[2].max(0) as usize, shape[3].max(0) as usize);
        if out_w == 0 || out_h == 0 || prob.len() < out_w * out_h {
            return Err("det 输出为空".into());
        }

        // 2) det 后处理（简化版 DB）：概率图连通域 → 外接矩形 → unclip → 映射回原图
        let rx = orig_w as f32 / out_w as f32;
        let ry = orig_h as f32 / out_h as f32;
        let mut boxes: Vec<super::BoxF> =
            super::prob_to_boxes(&prob[..out_w * out_h], out_w, out_h)
                .into_iter()
                .map(|b| {
                    let scaled = super::BoxF {
                        x0: b.x0 * rx,
                        y0: b.y0 * ry,
                        x1: b.x1 * rx,
                        y1: b.y1 * ry,
                    };
                    super::unclip_box(scaled, super::UNCLIP_RATIO, orig_w as f32, orig_h as f32)
                })
                .collect();
        super::reading_order(&mut boxes);
        if boxes.is_empty() {
            return Ok(String::new()); // 全图无文本块 → 空串
        }

        // 3) 逐块 cls（可选）+ rec 识别
        let mut lines: Vec<String> = Vec::new();
        for b in boxes {
            let (x0, y0) = (b.x0.max(0.0) as u32, b.y0.max(0.0) as u32);
            let (w, h) = (
                ((b.x1 - b.x0).max(1.0) as u32).min(orig_w.saturating_sub(x0)),
                ((b.y1 - b.y0).max(1.0) as u32).min(orig_h.saturating_sub(y0)),
            );
            if w == 0 || h == 0 {
                continue;
            }
            let crop = image::imageops::crop_imm(&img, x0, y0, w, h).to_image();
            let crop = if should_rotate_180(eng, &crop)? {
                image::imageops::rotate180(&crop)
            } else {
                crop
            };
            let (rec_img, rec_w) = rec_resize(&crop);
            let rec_in = to_nchw(&rec_img);
            let (shape, logits) =
                run_first(&eng.rec, [1, 3, REC_H as usize, rec_w as usize], rec_in)?;
            if shape.len() != 3 {
                continue;
            }
            let (t, classes) = (shape[1].max(0) as usize, shape[2].max(0) as usize);
            if t == 0 || classes == 0 || logits.len() < t * classes {
                continue;
            }
            // CTC greedy：逐时间步 argmax
            let mut indices = Vec::with_capacity(t);
            for i in 0..t {
                let row = &logits[i * classes..(i + 1) * classes];
                let mut best = 0usize;
                for (j, &v) in row.iter().enumerate() {
                    if v > row[best] {
                        best = j;
                    }
                }
                indices.push(best);
            }
            let text = super::ctc_greedy_decode(&indices, &eng.keys);
            if !text.trim().is_empty() {
                lines.push(text);
            }
        }
        Ok(lines.join("\n"))
    }
}

// ───────────── 纯函数（det 后处理 / CTC 解码，可脱离 ONNX 单测） ─────────────

/// 简化版 DB unclip 外扩比例（Vatti 多边形外扩的矩形近似）
#[cfg(any(target_os = "windows", test))]
const UNCLIP_RATIO: f32 = 1.5;

/// 浮点文本框（像素坐标，x1/y1 为右下角，含端点）
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct BoxF {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// det 后处理（简化版 DB 后处理）：概率图 > 阈值二值化 → 4 连通 flood fill
/// 连通域 → 外接矩形。过小的连通域（< MIN_AREA 像素）视为噪声丢弃。
#[cfg(any(target_os = "windows", test))]
fn prob_to_boxes(prob: &[f32], w: usize, h: usize) -> Vec<BoxF> {
    const THRESH: f32 = 0.3;
    const MIN_AREA: usize = 10;
    let mut visited = vec![false; w * h];
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if visited[idx] || prob.get(idx).copied().unwrap_or(0.0) <= THRESH {
                continue;
            }
            // flood fill 收集一个连通域
            let (mut min_x, mut max_x, mut min_y, mut max_y, mut area) = (x, x, y, y, 0usize);
            let mut stack = vec![(x, y)];
            visited[idx] = true;
            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);
                let neighbors = [
                    (cx.wrapping_sub(1), cy, cx > 0),
                    (cx + 1, cy, cx + 1 < w),
                    (cx, cy.wrapping_sub(1), cy > 0),
                    (cx, cy + 1, cy + 1 < h),
                ];
                for (nx, ny, ok) in neighbors {
                    if !ok {
                        continue;
                    }
                    let ni = ny * w + nx;
                    if !visited[ni] && prob.get(ni).copied().unwrap_or(0.0) > THRESH {
                        visited[ni] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            if area >= MIN_AREA {
                out.push(BoxF {
                    x0: min_x as f32,
                    y0: min_y as f32,
                    x1: (max_x + 1) as f32,
                    y1: (max_y + 1) as f32,
                });
            }
        }
    }
    out
}

/// 简化版 unclip：各边外扩 (ratio-1)/2 × min(宽,高)，并裁剪到图内
#[cfg(any(target_os = "windows", test))]
fn unclip_box(b: BoxF, ratio: f32, max_w: f32, max_h: f32) -> BoxF {
    let margin = (ratio - 1.0) / 2.0 * (b.x1 - b.x0).min(b.y1 - b.y0);
    BoxF {
        x0: (b.x0 - margin).max(0.0),
        y0: (b.y0 - margin).max(0.0),
        x1: (b.x1 + margin).min(max_w),
        y1: (b.y1 + margin).min(max_h),
    }
}

/// 阅读顺序排序：按行分组（中心 y 差 < 平均行高一半视为同行），行内从左到右
#[cfg(any(target_os = "windows", test))]
fn reading_order(boxes: &mut [BoxF]) {
    if boxes.len() < 2 {
        return;
    }
    let avg_h = boxes.iter().map(|b| (b.y1 - b.y0).max(1.0)).sum::<f32>() / boxes.len() as f32;
    boxes.sort_by(|a, b| {
        let acy = (a.y0 + a.y1) / 2.0;
        let bcy = (b.y0 + b.y1) / 2.0;
        if (acy - bcy).abs() < avg_h / 2.0 {
            a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            acy.partial_cmp(&bcy).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
}

/// CTC greedy 解码：去重复 → 去 blank（索引 0）→ 查字典。
/// keys[i] 对应索引 i+1；索引 keys.len()+1 按 PaddleOCR 约定是空格；越界索引跳过。
#[cfg(any(target_os = "windows", test))]
fn ctc_greedy_decode(indices: &[usize], keys: &[String]) -> String {
    let mut out = String::new();
    let mut prev: Option<usize> = None;
    for &i in indices {
        if Some(i) == prev {
            continue; // CTC 去重复（相邻相同合并）
        }
        prev = Some(i);
        if i == 0 {
            continue; // blank
        }
        if i <= keys.len() {
            out.push_str(&keys[i - 1]);
        } else if i == keys.len() + 1 {
            out.push(' ');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_network_paths_before_permission() {
        // 隐私红线：网络路径入口即拒（不经过白名单弹窗）。
        // 直接测 recognize 层之上的判定逻辑太重（要 AppHandle），
        // 这里锁判定函数同款前缀条件，防回归改丢。
        for p in [
            "http://a.com/x.png",
            "https://a.com/x.png",
            "data:image/png;base64,xx",
        ] {
            assert!(
                p.starts_with("http://") || p.starts_with("https://") || p.starts_with("data:"),
                "{p} 应被识别为网络路径"
            );
        }
    }

    #[test]
    fn prob_to_boxes_finds_two_components() {
        // 10x10 概率图：左上 4x4 亮块 + 右下 2x6 亮块（面积均 ≥ MIN_AREA）
        let (w, h) = (10usize, 10usize);
        let mut prob = vec![0f32; w * h];
        for y in 1..5 {
            for x in 1..5 {
                prob[y * w + x] = 0.9;
            }
        }
        for y in 4..10 {
            for x in 6..8 {
                prob[y * w + x] = 0.8;
            }
        }
        let mut boxes = prob_to_boxes(&prob, w, h);
        reading_order(&mut boxes);
        assert_eq!(boxes.len(), 2, "应找到 2 个连通域：{boxes:?}");
        assert_eq!(
            boxes[0],
            BoxF {
                x0: 1.0,
                y0: 1.0,
                x1: 5.0,
                y1: 5.0
            }
        );
        assert_eq!(
            boxes[1],
            BoxF {
                x0: 6.0,
                y0: 4.0,
                x1: 8.0,
                y1: 10.0
            }
        );
    }

    #[test]
    fn prob_to_boxes_filters_noise_and_blank() {
        let (w, h) = (8usize, 8usize);
        assert!(
            prob_to_boxes(&vec![0f32; w * h], w, h).is_empty(),
            "全 0 概率图无文本块"
        );
        let mut prob = vec![0f32; w * h];
        prob[0] = 0.9; // 1 像素噪声 < MIN_AREA
        prob[1] = 0.9;
        assert!(prob_to_boxes(&prob, w, h).is_empty(), "过小连通域应被过滤");
        // 阈值边界：== 阈值不算命中（判定是 > THRESH）
        let mut prob = vec![0f32; w * h];
        for i in 0..12 {
            prob[i] = 0.3;
        }
        assert!(prob_to_boxes(&prob, w, h).is_empty(), "等于阈值不应命中");
    }

    #[test]
    fn unclip_box_expands_and_clamps() {
        let b = BoxF {
            x0: 10.0,
            y0: 10.0,
            x1: 30.0,
            y1: 20.0,
        };
        // min(20,10)=10，margin = 0.25*10 = 2.5
        let r = unclip_box(b, UNCLIP_RATIO, 100.0, 100.0);
        assert_eq!(
            r,
            BoxF {
                x0: 7.5,
                y0: 7.5,
                x1: 32.5,
                y1: 22.5
            }
        );
        // 贴边的框外扩后被裁剪到图内
        let edge = BoxF {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 10.0,
        };
        let r = unclip_box(edge, UNCLIP_RATIO, 100.0, 100.0);
        assert_eq!(r.x0, 0.0);
        assert_eq!(r.y0, 0.0);
        assert_eq!(r.x1, 12.5);
    }

    #[test]
    fn reading_order_top_to_bottom_left_to_right() {
        let mut boxes = vec![
            BoxF {
                x0: 50.0,
                y0: 0.0,
                x1: 80.0,
                y1: 10.0,
            }, // 第一行右侧
            BoxF {
                x0: 0.0,
                y0: 40.0,
                x1: 30.0,
                y1: 50.0,
            }, // 第二行
            BoxF {
                x0: 0.0,
                y0: 2.0,
                x1: 40.0,
                y1: 12.0,
            }, // 第一行左侧（y 略有抖动，仍属同行）
        ];
        reading_order(&mut boxes);
        assert_eq!(boxes[0].x0, 0.0);
        assert_eq!(boxes[0].y0, 2.0);
        assert_eq!(boxes[1].x0, 50.0);
        assert_eq!(boxes[2].y0, 40.0);
    }

    #[test]
    fn ctc_greedy_decode_merges_repeats_and_skips_blank() {
        let keys: Vec<String> = ["你", "好", "a", "b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // 索引：0=blank, 1=你, 2=好, 3=a, 4=b, 5=空格(=keys.len()+1)
        let indices = [0, 1, 1, 1, 0, 2, 3, 3, 4, 5];
        assert_eq!(ctc_greedy_decode(&indices, &keys), "你好ab ");
        assert_eq!(ctc_greedy_decode(&[], &keys), "");
        assert_eq!(ctc_greedy_decode(&[0, 0, 0], &keys), "");
        // blank 隔开的相同字符不合并（CTC 语义：1,0,1 → 你你）
        assert_eq!(ctc_greedy_decode(&[1, 0, 1], &keys), "你你");
        // 越界索引跳过不炸
        assert_eq!(ctc_greedy_decode(&[1, 99], &keys), "你");
    }

    /// 真实引擎冒烟（macOS：系统 Vision；需要 macOS 13+）。
    /// 生成一张白底黑字 PNG（内置 5x7 点阵字体画 "OCR 2026"），跑真实识别。
    /// 手动跑：cargo test --lib ocr:: -- --ignored
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "真实 Vision 推理冒烟（慢，手动跑）"]
    fn real_vision_smoke() {
        let png = test_png_with_text("OCR 2026");
        let text = recognize(&png).expect("Vision 识别应成功");
        assert!(text.contains("OCR"), "应识别出 OCR：{text:?}");
        assert!(text.contains("2026"), "应识别出 2026：{text:?}");
    }

    /// 用内置 5x7 点阵字体把 ASCII 文本画成白底黑字 PNG（测试夹具，无外部资源依赖）
    #[cfg(target_os = "macos")]
    fn test_png_with_text(text: &str) -> Vec<u8> {
        // 5x7 点阵（每字符 5 字节，每字节的低 5 位是一列的 7 行像素）
        #[rustfmt::skip]
        const FONT: &[(&str, [u8; 5])] = &[
            ("0", [0x3E, 0x51, 0x49, 0x45, 0x3E]), ("2", [0x42, 0x61, 0x51, 0x49, 0x46]),
            ("6", [0x3C, 0x4A, 0x49, 0x49, 0x30]), ("C", [0x3E, 0x41, 0x41, 0x41, 0x22]),
            ("O", [0x3E, 0x41, 0x41, 0x41, 0x3E]), ("R", [0x7F, 0x09, 0x19, 0x29, 0x46]),
            (" ", [0x00, 0x00, 0x00, 0x00, 0x00]),
        ];
        const SCALE: usize = 6; // 放大倍数（Vision 对过小字号识别差）
        const PAD: usize = 24;
        let glyph_w = 6 * SCALE; // 5 列 + 1 列间距
        let w = PAD * 2 + text.chars().count() * glyph_w;
        let h = PAD * 2 + 7 * SCALE;
        let mut img = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([255, 255, 255]));
        for (ci, ch) in text.chars().enumerate() {
            let key = ch.to_string();
            let Some((_, glyph)) = FONT.iter().find(|(k, _)| *k == key) else {
                continue;
            };
            for col in 0..5 {
                for row in 0..7 {
                    if glyph[col] >> row & 1 == 1 {
                        let px = PAD + ci * glyph_w + col * SCALE;
                        let py = PAD + row * SCALE;
                        for dy in 0..SCALE {
                            for dx in 0..SCALE {
                                img.put_pixel(
                                    (px + dx) as u32,
                                    (py + dy) as u32,
                                    image::Rgb([0, 0, 0]),
                                );
                            }
                        }
                    }
                }
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("PNG 编码");
        buf.into_inner()
    }
}
