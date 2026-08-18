//! Python 执行基础设施：调用本机 Python 处理文档 / 运行模型生成的脚本。
//!
//! 安全设计（对齐 Harness 网关）：
//! - 开关：py-enabled.flag（设置页「允许机器人执行 Python」，默认关闭）——主要防护
//! - 受限执行：每次运行独立临时目录（数据目录 py-runs/<uuid>/），只经 stdin/文件传参，永不拼 shell；
//!   ⚠️ 不是安全沙箱：脚本以当前用户完整权限运行（可读本机文件、可联网），仅隔离工作目录
//! - 熔断：默认超时 60s 强杀（含子进程组）；stdout/stderr 读取时硬截断 64KB
//! - 审计：bot.log 记录脚本摘要、耗时、退出码、输出摘要
//! - 固定脚本模板：文档处理用预写脚本（extract/make_docx/make_xlsx），模型只填参数；
//!   自由编程走 run_python（需开关开启）

use serde::Serialize;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::AppHandle;

// ───────────────────────── 开关 ─────────────────────────

fn py_flag_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("py-enabled.flag")
}

#[tauri::command]
pub fn py_get_enabled(app: AppHandle) -> bool {
    py_flag_path(&app).exists()
}

#[tauri::command]
pub fn py_set_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if enabled {
        std::fs::write(py_flag_path(&app), b"1").map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(py_flag_path(&app));
    }
    Ok(enabled)
}

// ───────────────────────── 环境检测 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyEnv {
    pub available: bool,
    pub python: String,
    pub version: String,
    pub libs: Vec<String>,
}

/// 检测本机 Python（macOS/Linux: python3/python；Windows: python/python3/py -3）
pub fn detect_python() -> Option<String> {
    #[cfg(windows)]
    let candidates: &[&str] = &["python", "python3"];
    #[cfg(not(windows))]
    let candidates: &[&str] = &["python3", "python"];
    for c in candidates {
        if let Ok(out) = Command::new(c).arg("--version").output() {
            if out.status.success() {
                return Some(c.to_string());
            }
        }
    }
    // Windows 兜底：py 启动器
    #[cfg(windows)]
    {
        if let Ok(out) = Command::new("py").args(["-3", "--version"]).output() {
            if out.status.success() {
                return Some("py".to_string());
            }
        }
    }
    None
}

/// 同步检测核心（后台线程运行）
fn py_env_check_blocking() -> PyEnv {
    let Some(py) = detect_python() else {
        return PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        };
    };
    let version = Command::new(&py)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // 单进程一次探测全部库（原 4 次独立 spawn 冷启动，同步命令下卡主线程几秒）
    let probe = r#"import importlib
for m in ["openpyxl", "docx", "pptx", "pypdf", "reportlab"]:
    try:
        importlib.import_module(m)
        print(f"{m}:OK")
    except Exception:
        print(f"{m}:缺")
"#;
    let mut libs = Vec::new();
    if let Ok(out) = Command::new(&py).args(["-c", probe]).output() {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if line.contains(':') {
                    libs.push(line.to_string());
                }
            }
        }
    }
    PyEnv {
        available: true,
        python: py,
        version,
        libs,
    }
}

/// 检查本机 Python 环境：后台线程执行，不阻塞主线程（审计：同步命令会冻结 UI）
#[tauri::command]
pub async fn py_env_check() -> PyEnv {
    tauri::async_runtime::spawn_blocking(py_env_check_blocking)
        .await
        .unwrap_or_else(|_| PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        })
}

// ───────────────────────── 执行核心 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyRunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
}

const OUTPUT_CAP: usize = 64 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// 终止 Python 进程及其全部子进程：Unix 按进程组（spawn 时 process_group(0) 成为组首），
/// Windows 用 taskkill /T 树杀。先组杀再兜底 kill + wait。
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .status();
    }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn truncate_output(s: String) -> String {
    let count = s.chars().count();
    if count <= OUTPUT_CAP {
        s
    } else {
        let mut out: String = s.chars().take(OUTPUT_CAP).collect();
        out.push_str("\n…（输出已截断）");
        out
    }
}

/// 执行一段 Python 脚本（写入独立临时目录运行）。
/// `input_json`：可选，写入 params.json 供脚本读取；`args`：附加命令行参数。
pub fn run_python(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
) -> Result<PyRunResult, String> {
    let Some(py) = detect_python() else {
        return Err("本机未检测到 Python。macOS 请安装 Command Line Tools；Windows 请到 python.org 安装并勾选 Add to PATH".into());
    };
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    // 独立临时目录
    let dir = crate::db::data_dir(app)
        .join("py-runs")
        .join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("run.py"), script).map_err(|e| e.to_string())?;
    if let Some(j) = input_json {
        std::fs::write(dir.join("params.json"), j).map_err(|e| e.to_string())?;
    }

    let mut cmd = Command::new(&py);
    // Unix：子进程自成进程组（组首），超时可整组强杀，不残留孙进程
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.arg("run.py")
        .args(args)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("启动 Python 失败：{e}"))?;

    // 双线程读输出防管道死锁（stdout/stderr 先取出再交给线程）
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    let tx_out = tx.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stdout {
            // 读取时硬截断（审计 P1：原先 read_to_end 无上限，失控脚本 60s 可刷出数百 MB）
            let _ = s.take((OUTPUT_CAP + 1024) as u64).read_to_end(&mut buf);
        }
        let _ = tx_out.send(("out", buf));
    });
    let tx_err = tx.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stderr {
            let _ = s.take((OUTPUT_CAP + 1024) as u64).read_to_end(&mut buf);
        }
        let _ = tx_err.send(("err", buf));
    });
    drop(tx);

    let start = Instant::now();
    let exit_code = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status.code();
        }
        if start.elapsed() > timeout {
            kill_tree(&mut child);
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "执行超时（{timeout_secs}s）已强制终止",
                timeout_secs = timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    for (kind, buf) in rx.iter() {
        let text = String::from_utf8_lossy(&buf).into_owned();
        if kind == "out" {
            stdout = text;
        } else {
            stderr = text;
        }
    }
    let duration_ms = start.elapsed().as_millis();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(PyRunResult {
        stdout: truncate_output(stdout),
        stderr: truncate_output(stderr),
        exit_code,
        duration_ms,
    })
}

/// 审计日志钩子（bot.rs 的 audit_log 已存在，这里复用数据目录 bot.log）
pub fn py_audit(app: &AppHandle, line: &str) {
    let p = crate::db::data_dir(app).join("bot.log");
    crate::db::rotate_log_if_large(&p, 5 * 1024 * 1024);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {line}");
    }
}

// ───────────────────────── 固定文档脚本模板 ─────────────────────────

/// 提取文档文本（docx/xlsx/pptx/pdf），stdin 读 params.json {path}
pub const EXTRACT_SCRIPT: &str = r#"import json, sys, os
p = json.load(open('params.json', encoding='utf-8'))['path']
ext = os.path.splitext(p)[1].lower()
if not os.path.exists(p):
    print('文件不存在：' + p); sys.exit(1)
if ext == '.docx':
    import docx
    d = docx.Document(p)
    parts = [para.text for para in d.paragraphs if para.text.strip()]
    for tbl in d.tables:
        for row in tbl.rows:
            cells = [c.text.strip() for c in row.cells]
            parts.append(' | '.join(cells))
    print('\n'.join(parts))
elif ext == '.xlsx':
    import openpyxl
    wb = openpyxl.load_workbook(p, data_only=True)
    for ws in wb.worksheets:
        print(f'=== 工作表：{ws.title} ===')
        for row in ws.iter_rows(values_only=True):
            vals = ['' if v is None else str(v) for v in row]
            if any(v.strip() for v in vals):
                print(' | '.join(vals))
elif ext == '.pptx':
    from pptx import Presentation
    prs = Presentation(p)
    for i, slide in enumerate(prs.slides, 1):
        print(f'=== 第{i}页 ===')
        for shape in slide.shapes:
            if shape.has_text_frame:
                t = shape.text_frame.text.strip()
                if t: print(t)
elif ext == '.pdf':
    from pypdf import PdfReader
    r = PdfReader(p)
    for i, page in enumerate(r.pages, 1):
        print(f'=== 第{i}页 ===')
        print(page.extract_text() or '')
else:
    print('不支持的格式：' + ext); sys.exit(1)
"#;

/// 生成 Word：stdin 读 params.json {title, paragraphs: [..], out}
pub const MAKE_DOCX_SCRIPT: &str = r#"import json, os
import docx
from docx.shared import Pt, Cm
p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
paras = p.get('paragraphs', [])
out = p['out']
d = docx.Document()
# 正文基础样式：宋体 12pt
style = d.styles['Normal']
style.font.name = '宋体'
style.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)
for para in paras:
    if para == '':
        d.add_paragraph('')
    else:
        pr = d.add_paragraph(para)
        # 正文首行缩进两字符（12pt × 2 = 24pt ≈ 0.85cm，公文排版规范）
        pr.paragraph_format.first_line_indent = Pt(24)
d.save(out)
print('已生成：' + out)
"#;

/// 生成修订模式 Word（track changes）：stdin 读 params.json {title, original_path, original, revised, out}
/// 原文优先回读文件（保真）；提取被截断时回退模型传的原文行，保证对比范围一致
pub const MAKE_DOCX_REVISIONS_SCRIPT: &str = r#"import json, os, sys, difflib
from datetime import datetime, timezone
import docx
from docx.shared import Pt
from docx.oxml import OxmlElement
from docx.oxml.ns import qn

p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
path = p.get('original_path', '')
orig_model = p.get('original') or []
rev = p.get('revised', [])
out = p['out']

if not rev:
    print('revised 不能为空'); sys.exit(1)

# 原文：优先从文件回读（与提取脚本同一套逻辑），读不到/长度超出模型所见时用模型传的原文行
orig_file = []
if path and os.path.exists(path) and path.lower().endswith('.docx'):
    d0 = docx.Document(path)
    orig_file = [para.text for para in d0.paragraphs if para.text.strip()]
    for tbl in d0.tables:
        for row in tbl.rows:
            cells = [c.text.strip() for c in row.cells]
            orig_file.append(' | '.join(cells))
if orig_file and (not orig_model or len(orig_file) <= len(orig_model)):
    orig = orig_file
    src_note = '原文取自文件'
elif orig_model:
    orig = orig_model
    src_note = '原文取自提取列表'
else:
    print('缺少原文：请提供 original_path（Word 路径）或 original（原文行列表）'); sys.exit(1)

AUTHOR = 'WMessage AI'
DATE = datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')

d = docx.Document()
st = d.styles['Normal']
st.font.name = '宋体'
st.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)

_id = [1000]
def nid():
    _id[0] += 1
    return _id[0]

def run_el(text, kind):
    r = OxmlElement('w:r')
    rPr = OxmlElement('w:rPr')
    rf = OxmlElement('w:rFonts')
    rf.set(qn('w:ascii'), '宋体')
    rf.set(qn('w:hAnsi'), '宋体')
    rf.set(qn('w:eastAsia'), '宋体')
    rPr.append(rf)
    sz = OxmlElement('w:sz'); sz.set(qn('w:val'), '24')
    szcs = OxmlElement('w:szCs'); szcs.set(qn('w:val'), '24')
    rPr.append(sz); rPr.append(szcs)
    if kind == 'del':
        rPr.append(OxmlElement('w:strike'))
        c = OxmlElement('w:color'); c.set(qn('w:val'), 'C00000'); rPr.append(c)
    elif kind == 'ins':
        u = OxmlElement('w:u'); u.set(qn('w:val'), 'single'); rPr.append(u)
        c = OxmlElement('w:color'); c.set(qn('w:val'), 'C00000'); rPr.append(c)
    r.append(rPr)
    t = OxmlElement('w:delText' if kind == 'del' else 'w:t')
    t.set(qn('xml:space'), 'preserve')
    t.text = text
    r.append(t)
    return r

def append_change(para, kind, text):
    el = OxmlElement('w:ins' if kind == 'ins' else 'w:del')
    el.set(qn('w:id'), str(nid()))
    el.set(qn('w:author'), AUTHOR)
    el.set(qn('w:date'), DATE)
    el.append(run_el(text, kind))
    para._p.append(el)

def diff_into(para, a, b):
    sm = difflib.SequenceMatcher(None, a, b)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            if i2 > i1:
                para._p.append(run_el(a[i1:i2], 'plain'))
        elif tag == 'delete':
            if i2 > i1:
                append_change(para, 'del', a[i1:i2])
        elif tag == 'insert':
            if j2 > j1:
                append_change(para, 'ins', b[j1:j2])
        else:
            if i2 > i1:
                append_change(para, 'del', a[i1:i2])
            if j2 > j1:
                append_change(para, 'ins', b[j1:j2])

# 段落级对齐（autojunk=False 防长列表相似段落被吞）
sm = difflib.SequenceMatcher(None, orig, rev, autojunk=False)
for tag, i1, i2, j1, j2 in sm.get_opcodes():
    if tag == 'equal':
        for t in orig[i1:i2]:
            para = d.add_paragraph()
            if t:
                para._p.append(run_el(t, 'plain'))
    elif tag == 'delete':
        for t in orig[i1:i2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'del', t)
    elif tag == 'insert':
        for t in rev[j1:j2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'ins', t)
    else:
        n = min(i2 - i1, j2 - j1)
        for k in range(n):
            para = d.add_paragraph()
            diff_into(para, orig[i1 + k], rev[j1 + k])
        for t in orig[i1 + n:i2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'del', t)
        for t in rev[j1 + n:j2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'ins', t)

d.save(out)
print('已生成（修订模式，' + src_note + '）：' + out)
"#;

/// 生成 Excel：stdin 读 params.json {sheets: [{name, rows: [[..]]}], out}
/// 单元格以 = 开头的字符串按原生公式写入（SUM/IF/VLOOKUP 等）
pub const MAKE_XLSX_SCRIPT: &str = r#"import json
import openpyxl
p = json.load(open('params.json', encoding='utf-8'))
sheets = p.get('sheets', [])
out = p['out']
wb = openpyxl.Workbook()
wb.remove(wb.active)
for i, sh in enumerate(sheets):
    ws = wb.create_sheet(sh.get('name', f'Sheet{i+1}'))
    for row in sh.get('rows', []):
        ws.append([v if not (isinstance(v, str) and v.startswith('=')) else v for v in row])
wb.save(out)
print('已生成：' + out)
"#;

/// 生成 PDF：stdin 读 params.json {title, paragraphs: [..], out}（reportlab，中文字体兜底）
pub const MAKE_PDF_SCRIPT: &str = r#"import json
from reportlab.pdfgen import canvas
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
paras = p.get('paragraphs', [])
pdfmetrics.registerFont(UnicodeCIDFont('STSong-Light'))
c = canvas.Canvas(out, pagesize=A4)
w, h = A4
y = h - 60
if title:
    c.setFont('STSong-Light', 18)
    c.drawString(60, y, title)
    y -= 40
c.setFont('STSong-Light', 11)
for para in paras:
    # 简单按行折行（每行约 42 个中文字符）
    for i in range(0, len(para), 42):
        if y < 60:
            c.showPage()
            c.setFont('STSong-Light', 11)
            y = h - 60
        c.drawString(60, y, para[i:i+42])
        y -= 18
    y -= 8
c.save()
print('已生成：' + out)
"#;

/// 生成 PPT：stdin 读 params.json {title, slides: [{title, bullets: [..]}], out}（python-pptx 标准版式）
pub const MAKE_PPTX_SCRIPT: &str = r#"import json, datetime
from pptx import Presentation
from pptx.util import Inches, Pt
from pptx.dml.color import RGBColor
from pptx.enum.text import PP_ALIGN, MSO_ANCHOR

p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
slides = p.get('slides', [])
theme = p.get('theme', 'blue')

THEMES = {
    # 来源：color-font-skill 18 套配色精选（文字型 PPT 对比度适配后取色）。
    # band=大面积色块（章节页/标题条/表头，必深色配 bandtext）；accent=点缀（竖条/强调字）；alt=表格斑马纹。
    'blue':  dict(bg='FFFFFF', accent='2B2D42', text='2B2B2B', sub='6B6B6B', band='2B2D42', bandtext='FFFFFF', alt='F2F4F7'),   # 商务与权威
    'navy':  dict(bg='000814', accent='FFC300', text='F0F0F0', sub='9AA5B1', band='003566', bandtext='FFFFFF', alt='0F1E33'),    # 科技与夜景（深色）
    'teal':  dict(bg='FFFFFF', accent='006D77', text='2B2B2B', sub='6B6B6B', band='006D77', bandtext='FFFFFF', alt='F2F7F7'),   # 现代与健康
    'forest':dict(bg='FEFAE0', accent='606C38', text='283618', sub='7A7A5C', band='283618', bandtext='FEFAE0', alt='F6F1DA'),   # 自然与户外
    'wine':  dict(bg='FDF0D5', accent='780000', text='2B2B2B', sub='8A6A4A', band='780000', bandtext='FFFFFF', alt='F9E8C8'),   # 复古与学院
    'sky':   dict(bg='FFFFFF', accent='0077B6', text='03045E', sub='6B6B6B', band='03045E', bandtext='FFFFFF', alt='F0F6FB'),   # 纯净科技蓝
    'plum':  dict(bg='F2E9E4', accent='22223B', text='2B2B2B', sub='8A8A8A', band='22223B', bandtext='FFFFFF', alt='EBE0D8'),   # 轻奢与神秘
    'coral': dict(bg='FDFCDC', accent='0081A7', text='2B2B2B', sub='6B6B6B', band='0081A7', bandtext='FFFFFF', alt='F8F5D2'),   # 海岸珊瑚
    'dark':  dict(bg='1E1E1E', accent='4A90D9', text='F0F0F0', sub='B0B0B0', band='2D2D2D', bandtext='FFFFFF', alt='2D2D2D'),   # 深色通用
    'green': dict(bg='FFFFFF', accent='1E7145', text='2B2B2B', sub='6B6B6B', band='1E7145', bandtext='FFFFFF', alt='F2F6F2'),   # 清新绿
}
T = THEMES.get(theme, THEMES['blue'])
C = lambda h: RGBColor.from_string(h)

prs = Presentation()
prs.slide_width = Inches(13.333)
prs.slide_height = Inches(7.5)
BLANK = prs.slide_layouts[6]
W, H = prs.slide_width, prs.slide_height

def rect(s, x, y, w, h, fill, line=None):
    from pptx.enum.shapes import MSO_SHAPE
    sp = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, x, y, w, h)
    sp.fill.solid(); sp.fill.fore_color.rgb = C(fill)
    if line is None:
        sp.line.fill.background()
    else:
        sp.line.color.rgb = C(line)
    sp.shadow.inherit = False
    return sp

def textbox(s, x, y, w, h, runs, size, color, bold=False, align=PP_ALIGN.LEFT,
            anchor=MSO_ANCHOR.TOP, line_spacing=1.1):
    """runs: str 或 [(text, dict)]；dict 可含 size/color/bold/italic"""
    tb = s.shapes.add_textbox(x, y, w, h)
    tf = tb.text_frame
    tf.word_wrap = True
    tf.vertical_anchor = anchor
    if isinstance(runs, str):
        runs = [(runs, {})]
    else:
        runs = [(str(r), {}) if not isinstance(r, (tuple, list)) else r for r in runs]
    for i, (txt, st) in enumerate(runs):
        para = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
        para.alignment = st.get('align', align)
        para.line_spacing = st.get('line_spacing', line_spacing)
        r = para.add_run(); r.text = txt
        f = r.font
        f.size = Pt(st.get('size', size))
        f.bold = st.get('bold', bold)
        f.color.rgb = C(st.get('color', color))
        f.name = 'Microsoft YaHei'
        try:
            # 中文字符需设置 ea typeface（latin 字体只对西文生效）
            from lxml import etree
            rPr = r._r.get_or_add_rPr()
            ea = rPr.find('{http://schemas.openxmlformats.org/drawingml/2006/main}ea')
            if ea is None:
                ea = etree.SubElement(rPr, '{http://schemas.openxmlformats.org/drawingml/2006/main}ea')
            ea.set('typeface', 'Microsoft YaHei')
        except Exception:
            pass
    return tb

def page_number(s, idx, total):
    textbox(s, W - Inches(1.2), H - Inches(0.5), Inches(0.9), Inches(0.3),
            f'{idx} / {total}', 10, T['sub'], align=PP_ALIGN.RIGHT)

def bullet_font_size(n):
    if n <= 5: return 20
    if n <= 8: return 16
    return 13

total = len(slides)

def render_cover(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.28), H, T['accent'])
    main = sl.get('title') or title or '演示文稿'
    sub = sl.get('subtitle', '')
    textbox(s, Inches(1.1), Inches(2.4), Inches(11), Inches(1.6),
            main, 48, T['text'], bold=True)
    if sub:
        textbox(s, Inches(1.15), Inches(4.2), Inches(11), Inches(0.8),
                sub, 20, T['sub'])
    textbox(s, Inches(1.15), Inches(6.5), Inches(6), Inches(0.5),
            datetime.date.today().strftime('%Y年%m月%d日'), 12, T['sub'])

def render_toc(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, W, Inches(1.1), T['band'])
    textbox(s, Inches(0.9), Inches(0.25), Inches(11.5), Inches(0.7),
            sl.get('title', '目录'), 30, T['bandtext'], bold=True)
    items = sl.get('items') or [b.get('title', '') for b in slides if b.get('type') != 'cover']
    y = Inches(1.7)
    for i, it in enumerate(items, 1):
        textbox(s, Inches(1.5), y, Inches(10.3), Inches(0.55),
                f'{i:02d}   {it}', 18, T['text'], bold=(i <= 9))
        y += Inches(0.62)

def render_section(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['band'])
    textbox(s, Inches(1.0), Inches(2.8), Inches(11.3), Inches(1.4),
            sl.get('title', ''), 40, T['bandtext'], bold=True, align=PP_ALIGN.CENTER)
    sub = sl.get('subtitle', '')
    if sub:
        textbox(s, Inches(1.0), Inches(4.5), Inches(11.3), Inches(0.7),
                sub, 18, T['bandtext'], align=PP_ALIGN.CENTER)

def render_content(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.18), H, T['accent'])
    textbox(s, Inches(0.85), Inches(0.5), Inches(11.6), Inches(0.9),
            sl.get('title', ''), 32, T['text'], bold=True)
    bullets = sl.get('bullets', [])
    n = len(bullets)
    size = bullet_font_size(n)
    two_col = n > 7
    if not two_col:
        textbox(s, Inches(1.05), Inches(1.7), Inches(11.3), Inches(5.1),
                bullets, size, T['text'], line_spacing=1.35)
    else:
        half = (n + 1) // 2
        for col, part in enumerate([bullets[:half], bullets[half:]]):
            x = Inches(1.05) + col * Inches(5.75)
            textbox(s, x, Inches(1.7), Inches(5.5), Inches(5.1),
                    part, size, T['text'], line_spacing=1.3)

def render_table(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.18), H, T['accent'])
    textbox(s, Inches(0.85), Inches(0.5), Inches(11.6), Inches(0.9),
            sl.get('title', ''), 32, T['text'], bold=True)
    rows = sl.get('rows', [])
    if not rows:
        return
    nr, nc = len(rows), max(len(r) for r in rows)
    from pptx.util import Inches as I_
    tbl_w, tbl_h = Inches(11.6), Inches(4.9)
    x, y = Inches(0.9), Inches(1.7)
    gfx = s.shapes.add_table(nr, nc, x, y, tbl_w, tbl_h)
    table = gfx.table
    for ri, row in enumerate(rows):
        for ci in range(nc):
            cell = table.cell(ri, ci)
            cell.text = str(row[ci]) if ci < len(row) else ''
            for para in cell.text_frame.paragraphs:
                for r in para.runs:
                    r.font.size = Pt(14 if ri else 15)
                    r.font.bold = (ri == 0)
                    r.font.color.rgb = C(T['bandtext'] if ri == 0 else T['text'])
                    r.font.name = 'Microsoft YaHei'
            cell.fill.solid()
            cell.fill.fore_color.rgb = C(T['band'] if ri == 0 else T['alt'])

def render_closing(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, H - Inches(0.28), W, Inches(0.28), T['accent'])
    textbox(s, Inches(1.0), Inches(2.9), Inches(11.3), Inches(1.2),
            sl.get('title', '谢谢'), 40, T['accent'], bold=True, align=PP_ALIGN.CENTER)
    sub = sl.get('subtitle', '')
    if sub:
        textbox(s, Inches(1.0), Inches(4.4), Inches(11.3), Inches(0.6),
                sub, 16, T['sub'], align=PP_ALIGN.CENTER)

def render_slide(sl):
    stype = sl.get('type', 'content')
    if stype == 'cover':
        render_cover(sl)
    elif stype == 'toc':
        render_toc(sl)
    elif stype == 'section':
        render_section(sl)
    elif stype == 'table':
        render_table(sl)
    elif stype == 'closing':
        render_closing(sl)
    else:
        render_content(sl)

body = [s for s in slides if s.get('type') != 'cover']
total_pages = len(body) + 1
cover = next((s for s in slides if s.get('type') == 'cover'),
             {'type': 'cover', 'title': title or '演示文稿', 'subtitle': ''})
render_slide(cover)
page_number(prs.slides[-1], 1, total_pages)
for i, sl in enumerate(body):
    render_slide(sl)
    page_number(prs.slides[-1], i + 2, total_pages)

prs.save(out)
print('已生成：' + out)

"#;

// ───────────────────────── 对外命令 ─────────────────────────

/// 执行同步核心（工具链在 async 上下文直接调用）
pub fn py_exec_sync(
    app: &AppHandle,
    code: String,
    timeout_secs: Option<u64>,
) -> Result<PyRunResult, String> {
    if !py_get_enabled(app.clone()) {
        return Err(
            "Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」".into(),
        );
    }
    py_audit(
        app,
        &format!("py_exec | script: {}", truncate_for_log(&code, 300)),
    );
    let r = run_python(app, &code, None, &[], timeout_secs)?;
    py_audit(
        app,
        &format!(
            "py_exec done | exit={:?} {}ms | out: {}",
            r.exit_code,
            r.duration_ms,
            truncate_for_log(&r.stdout, 300)
        ),
    );
    Ok(r)
}

/// 提取结果：文件路径 + 文本（修订模式需要原文路径回读原文）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocExtract {
    pub path: String,
    pub text: String,
}

/// 提取文档文本（弹框选文件或给定路径，按扩展名走固定脚本）
#[tauri::command]
pub async fn doc_extract(app: AppHandle, path: Option<String>) -> Result<DocExtract, String> {
    let path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            // 弹系统选择框由用户挑选（模型不知道路径，不得编造）
            let handle = app.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                use tauri_plugin_dialog::DialogExt;
                handle.dialog().file().blocking_pick_file()
            })
            .await
            .unwrap_or(None);
            match picked.and_then(file_path_to_string) {
                Some(p) => p,
                None => return Err("用户取消了选择".into()),
            }
        }
    };
    py_audit(&app, &format!("doc_extract | path: {path}"));
    let input = serde_json::json!({ "path": path }).to_string();
    let r = run_python(&app, EXTRACT_SCRIPT, Some(&input), &[], Some(120))?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_extract failed | {}", truncate_for_log(&r.stderr, 200)),
        );
        return Err(format!("提取失败：{}", r.stderr.trim()));
    }
    Ok(DocExtract {
        path,
        text: r.stdout,
    })
}

fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
    }
}

/// 生成 Word 到 AI_Gen_Files（不覆盖：同名自动加序号）
#[tauri::command]
pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_python(&app, MAKE_DOCX_SCRIPT, Some(&input), &[], Some(120))?;
    if r.exit_code != Some(0) {
        return Err(format!("生成 Word 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_word | out: {out}"));
    Ok(out)
}

/// 生成修订模式 Word（track changes）：回读原文与修订段落 diff，删除标删除线、新增标红色下划线，
/// 可在 Word 审阅中逐条接受/拒绝。original_path 优先回读文件保真；无路径时用 original 行列表。
#[tauri::command]
pub async fn doc_make_word_revisions(
    app: AppHandle,
    title: String,
    original_path: Option<String>,
    original: Vec<String>,
    revised: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input = serde_json::json!({
        "title": title,
        "original_path": original_path.clone().unwrap_or_default(),
        "original": original,
        "revised": revised,
        "out": out
    })
    .to_string();
    let r = run_python(
        &app,
        MAKE_DOCX_REVISIONS_SCRIPT,
        Some(&input),
        &[],
        Some(120),
    )?;
    if r.exit_code != Some(0) {
        return Err(format!("生成修订版 Word 失败：{}", r.stderr.trim()));
    }
    py_audit(
        &app,
        &format!(
            "doc_make_word_revisions | src: {} | out: {out}",
            original_path.unwrap_or_default()
        ),
    );
    Ok(out)
}

/// 生成 Excel 到 AI_Gen_Files（支持 =公式 单元格）
#[tauri::command]
pub async fn doc_make_excel(
    app: AppHandle,
    sheets: Vec<serde_json::Value>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "xlsx")?;
    let input = serde_json::json!({ "sheets": sheets, "out": out }).to_string();
    let r = run_python(&app, MAKE_XLSX_SCRIPT, Some(&input), &[], Some(120))?;
    if r.exit_code != Some(0) {
        return Err(format!("生成 Excel 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

/// 生成 PDF 到 AI_Gen_Files
#[tauri::command]
pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_python(&app, MAKE_PDF_SCRIPT, Some(&input), &[], Some(120))?;
    if r.exit_code != Some(0) {
        return Err(format!("生成 PDF 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

/// 生成 PPT 到 AI_Gen_Files
#[tauri::command]
pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    theme: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "pptx")?;
    let theme = theme
        .map(|t| t.trim().to_lowercase())
        .filter(|t| {
            matches!(
                t.as_str(),
                "blue"
                    | "navy"
                    | "teal"
                    | "forest"
                    | "wine"
                    | "sky"
                    | "plum"
                    | "coral"
                    | "dark"
                    | "green"
            )
        })
        .unwrap_or_else(|| "blue".into());
    let input = serde_json::json!({ "title": title, "slides": slides, "out": out, "theme": theme })
        .to_string();
    let r = run_python(&app, MAKE_PPTX_SCRIPT, Some(&input), &[], Some(120))?;
    if r.exit_code != Some(0) {
        return Err(format!("生成 PPT 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_ppt | out: {out}"));
    Ok(out)
}

/// 输出路径：AI_Gen_Files/<文件名>；同名自动加 (n) 序号，永不覆盖（Harness 第 6 层）
fn gen_out_path(app: &AppHandle, filename: Option<&str>, ext: &str) -> Result<String, String> {
    let dir = crate::db::data_dir(app).join("AI_Gen_Files");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 文件名只取 basename，防路径穿越
    let base = filename
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| format!("文档-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let base = base.trim_end_matches(&format!(".{ext}"));
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(candidate.to_string_lossy().to_string())
}

fn truncate_for_log(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}
