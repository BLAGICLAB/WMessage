//! 文档生成（Word / Excel / PDF / PPT）+ py_exec_sync 入口
//!
//! 包含 6 个固定 Python 脚本常量 + 5 个 doc_* tauri 命令 + py_exec_sync 同步入口。
//! 引擎优先 .NET OpenXML（修订版 Word），不可用回退 Python 脚本。

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::bot_slash::StopToken;
use crate::error::{CommandError, CommandResult};
use crate::py::audit::py_audit;
use crate::py::env::{run_dotnet_revisions, PyEnv};
use crate::py::runtime::{
    kill_all_py_children, mark_exiting, py_run_gate, py_runs_root, run_python, run_python_at,
    run_python_ungated, PyRunResult, RunFail, MAX_TIMEOUT_SECS, OUTPUT_CAP,
};

// ───────────── 固定文档脚本模板 ─────────────

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
    def walk(shapes):
        for shape in shapes:
            if shape.shape_type == 6:
                walk(shape.shapes)
            elif shape.has_table:
                for row in shape.table.rows:
                    cells = [c.text.strip() for c in row.cells]
                    print(' | '.join(cells))
            elif shape.has_text_frame:
                t = shape.text_frame.text.strip()
                if t: print(t)
    for i, slide in enumerate(prs.slides, 1):
        print(f'=== 第{i}页 ===')
        walk(slide.shapes)
elif ext == '.pdf':
    from pypdf import PdfReader
    r = PdfReader(p)
    for i, page in enumerate(r.pages, 1):
        print(f'=== 第{i}页 ===')
        print(page.extract_text() or '')
else:
    print('不支持的格式：' + ext); sys.exit(1)
"#;

pub const MAKE_DOCX_SCRIPT: &str = r#"import json, os
import docx
from docx.shared import Pt, Cm
p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
paras = p.get('paragraphs', [])
tables = p.get('tables', [])
out = p['out']
d = docx.Document()
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
        pr.paragraph_format.first_line_indent = Pt(24)
for t in tables:
    rows = t.get('rows', [])
    if not rows:
        continue
    if t.get('title'):
        tp = d.add_paragraph()
        tr = tp.add_run(t['title'])
        tr.font.bold = True
    tb = d.add_table(rows=len(rows), cols=len(rows[0]))
    tb.style = 'Table Grid'
    for i, row in enumerate(rows):
        for j, cell in enumerate(row):
            tb.cell(i, j).text = str(cell)
    for cell in tb.rows[0].cells:
        for cpara in cell.paragraphs:
            for run in cpara.runs:
                run.font.bold = True
    d.add_paragraph('')
d.save(out)
print('已生成：' + out)
"#;

// MAKE_DOCX_REVISIONS_SCRIPT：修订版 Word 的 Python 脚本——引擎优先 .NET，
// .NET 不可用或失败时回退走本脚本（run_doc_revisions 内的 fallback）。
pub const MAKE_DOCX_REVISIONS_SCRIPT: &str = r#"import json, os, sys, difflib, shutil, copy
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

AUTHOR = 'WMessage AI'
# 修订不写 w:date（修订日期不要了）
_id = [1000]
def nid():
    _id[0] += 1
    return _id[0]

# ──────────── 就地修订（保留原文格式）：w:ins 用 w:t、w:del 用 w:delText ────────────
# 文本口径（与 extract_document 的 python-docx para.text 严格对齐）：
# 只数直接子级 w:r 和 w:hyperlink 内的 run——w:t 原文、w:tab/w:ptab→\t、w:br/w:cr→\n、
# w:noBreakHyphen→'-'；域代码、已有修订等不计。若 run_text 只读 w:t 而单元文本用
# para.text（含 \t/\n/超链接文本），口径不一致会导致含 tab/换行/超链接的段落必中
# 「整段删+整段增」保底，看不出究竟改了哪几个字。

def run_text(r):
    parts = []
    for node in r:
        tag = node.tag
        if tag == qn('w:t'):
            parts.append(node.text or '')
        elif tag in (qn('w:tab'), qn('w:ptab')):
            parts.append('\t')
        elif tag in (qn('w:br'), qn('w:cr')):
            parts.append('\n')
        elif tag == qn('w:noBreakHyphen'):
            parts.append('-')
    return ''.join(parts)

def inner_runs(p_el):
    """段落文本载体 run：直接子级 w:r + w:hyperlink 内的 w:r（文档序，同 para.text 口径）"""
    rs = []
    for child in p_el:
        if child.tag == qn('w:r'):
            rs.append(child)
        elif child.tag == qn('w:hyperlink'):
            rs.extend(child.findall(qn('w:r')))
    return rs

def para_text(p_el):
    return ''.join(run_text(r) for r in inner_runs(p_el))

def clone_rpr(r):
    rpr = r.find(qn('w:rPr'))
    return copy.deepcopy(rpr) if rpr is not None else None

def fill_run_text(r, text, deleted=False):
    """\t→w:tab、\n→w:br，其余进 w:t（del 用 w:delText）：与提取口径互逆，
    equal 片段里的 tab/换行重建后不变形"""
    tag = 'w:delText' if deleted else 'w:t'
    i = 0
    for j in range(len(text) + 1):
        if j == len(text) or text[j] in '\t\n':
            if j > i:
                t = OxmlElement(tag)
                t.set(qn('xml:space'), 'preserve')
                t.text = text[i:j]
                r.append(t)
            if j < len(text):
                r.append(OxmlElement('w:tab' if text[j] == '\t' else 'w:br'))
            i = j + 1

def make_run(text, rpr, deleted=False):
    r = OxmlElement('w:r')
    if rpr is not None:
        r.append(copy.deepcopy(rpr))
    fill_run_text(r, text, deleted)
    return r

def wrap(kind, r):
    el = OxmlElement('w:ins' if kind == 'ins' else 'w:del')
    el.set(qn('w:id'), str(nid()))
    el.set(qn('w:author'), AUTHOR)
    el.append(r)
    return el

def unwrap_hyperlinks(p_el):
    """hyperlink 元素原位替换为其内部 run（rPr 保留 Hyperlink 样式外观）：
    修订标记不维护 hyperlink 嵌套，整段标删前先解包，避免链接文本漏标删"""
    for h in p_el.findall(qn('w:hyperlink')):
        idx = list(p_el).index(h)
        rs = h.findall(qn('w:r'))
        p_el.remove(h)
        for k, r in enumerate(rs):
            p_el.insert(idx + k, r)

def mark_para_deleted(p_el):
    """整段标删：含文本的 run 转 w:del（保留各 run 原 rPr），无文本 run（图片等）不动"""
    unwrap_hyperlinks(p_el)
    runs = p_el.findall(qn('w:r'))
    dels = []
    for r in runs:
        t = run_text(r)
        if t:
            dels.append(wrap('del', make_run(t, clone_rpr(r), deleted=True)))
    for r in runs:
        if run_text(r):
            p_el.remove(r)
    for el in dels:
        p_el.append(el)

def revise_para(p_el, old, new):
    """行内字符级 diff：equal 片段沿用原 run（克隆 rPr 拆段），del/ins 克隆锚点 rPr。
    run 映射与段落文本同一口径（para_text），正常情况下恒一致；段落含域代码/
    内容控件等口径外结构导致对不上时，才保底整段删+整段增。"""
    runs = inner_runs(p_el)
    pos = 0
    mp = []
    for r in runs:
        t = run_text(r)
        mp.append((pos, pos + len(t), r))
        pos += len(t)
    if pos != len(old) or ''.join(run_text(r) for _, _, r in mp) != old:
        anchor_rpr = clone_rpr(mp[-1][2]) if mp else None
        mark_para_deleted(p_el)
        if new:
            p_el.append(wrap('ins', make_run(new, anchor_rpr)))
        return
    def rpr_at(i):
        for s, e, r in mp:
            if s <= i < e:
                return clone_rpr(r)
        return clone_rpr(mp[-1][2]) if mp else None
    def pieces(a, b):
        for s, e, r in mp:
            lo, hi = max(a, s), min(b, e)
            if lo < hi:
                yield run_text(r)[lo - s:hi - s], clone_rpr(r)
    content = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, old, new).get_opcodes():
        if tag == 'equal':
            for piece, rpr in pieces(i1, i2):
                content.append(make_run(piece, rpr))
        elif tag == 'delete':
            for piece, rpr in pieces(i1, i2):
                content.append(wrap('del', make_run(piece, rpr, deleted=True)))
        elif tag == 'insert':
            content.append(wrap('ins', make_run(new[j1:j2], rpr_at(i1))))
        else:
            for piece, rpr in pieces(i1, i2):
                content.append(wrap('del', make_run(piece, rpr, deleted=True)))
            content.append(wrap('ins', make_run(new[j1:j2], rpr_at(i1))))
    # 重建：移除原文本载体（直接子级 run + hyperlink，连带其内 run），追加 diff 后的新内容
    for child in list(p_el):
        if child.tag in (qn('w:r'), qn('w:hyperlink')):
            p_el.remove(child)
    for el in content:
        p_el.append(el)

def insert_para_after(body_el, anchor_el, text, style_src=None):
    """锚点后插新段落（整段 w:ins）：pPr/rPr 克隆自 style_src 段落继承样式字体；
    无锚点插到正文开头（sectPr 之前）。返回新段落元素作下一个锚点。"""
    p = OxmlElement('w:p')
    rpr = None
    if style_src is not None:
        ppr = style_src.find(qn('w:pPr'))
        if ppr is not None:
            p.append(copy.deepcopy(ppr))
        for r in style_src.findall(qn('w:r')):
            if run_text(r):
                rpr = clone_rpr(r)
                break
    p.append(wrap('ins', make_run(text, rpr)))
    if anchor_el is None:
        sect = body_el.find(qn('w:sectPr'))
        if sect is not None:
            sect.addprevious(p)
        else:
            body_el.append(p)
    else:
        anchor_el.addnext(p)
    return p

def revise_in_place():
    d = docx.Document(out)  # out 已是原文档副本
    body_el = d.element.body
    # 对齐单元（与提取脚本同一口径）：正文非空段落文档序在前，表格行在后
    units = []
    for para in d.paragraphs:
        if para.text.strip():
            units.append(('p', para._p, para.text, para._p))
    for tbl in d.tables:
        for row in tbl.rows:
            units.append(('row', row._tr, ' | '.join(c.text.strip() for c in row.cells), tbl._tbl))
    orig = [u[2] for u in units]

    def anchor_of(i):
        kind, el, _, aux = units[i]
        return (el, aux if kind == 'p' else None)  # (锚点元素, 样式来源段落)

    def replace_unit(i, new_text):
        kind, el, old_text, aux = units[i]
        if kind == 'p':
            revise_para(el, old_text, new_text)
            return el
        # 表格行：按 " | " 拆回单元格逐格 diff；格数对不上整行标删 + 表后插新段落
        cells = el.findall(qn('w:tc'))
        parts = [x.strip() for x in new_text.split(' | ')]
        if len(parts) == len(cells):
            for tc, part in zip(cells, parts):
                paras = tc.findall(qn('w:p'))
                text_paras = [x for x in paras if para_text(x).strip()]
                if text_paras:
                    revise_para(text_paras[0], para_text(text_paras[0]), part)
                    for extra in text_paras[1:]:
                        mark_para_deleted(extra)
                elif paras:
                    paras[0].append(wrap('ins', make_run(part, None)))
            return aux
        for tc in cells:
            for x in tc.findall(qn('w:p')):
                mark_para_deleted(x)
        return insert_para_after(body_el, aux, new_text)

    sm = difflib.SequenceMatcher(None, orig, rev, autojunk=False)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            continue  # 原样不动：格式自然保留
        elif tag == 'delete':
            for i in range(i1, i2):
                kind, el, _, _2 = units[i]
                if kind == 'p':
                    mark_para_deleted(el)
                else:
                    for tc in el.findall(qn('w:tc')):
                        for x in tc.findall(qn('w:p')):
                            mark_para_deleted(x)
        elif tag == 'insert':
            anchor_el, style_src = anchor_of(i1 - 1) if i1 > 0 else (None, None)
            for j in range(j1, j2):
                if rev[j]:
                    anchor_el = insert_para_after(body_el, anchor_el, rev[j], style_src)
                    style_src = anchor_el
        else:
            n = min(i2 - i1, j2 - j1)
            anchor_el, style_src = anchor_of(i1 - 1) if i1 > 0 else (None, None)
            for k in range(n):
                anchor_el = replace_unit(i1 + k, rev[j1 + k])
                style_src = anchor_el if anchor_el.tag == qn('w:p') else None
            for i in range(i1 + n, i2):
                kind, el, _, _2 = units[i]
                if kind == 'p':
                    mark_para_deleted(el)
                else:
                    for tc in el.findall(qn('w:tc')):
                        for x in tc.findall(qn('w:p')):
                            mark_para_deleted(x)
                anchor_el, style_src = anchor_of(i)
            for j in range(j1 + n, j2):
                if rev[j]:
                    anchor_el = insert_para_after(body_el, anchor_el, rev[j], style_src)
                    style_src = anchor_el
    d.save(out)

if path and os.path.exists(path) and path.lower().endswith('.docx'):
    try:
        shutil.copyfile(path, out)
        revise_in_place()
        print('已生成（修订模式·保留原文格式，原文取自文件）：' + out)
        sys.exit(0)
    except Exception as ex:
        # 就地失败（文件损坏/结构异常）不硬挂：回退新建模式，保证有产物
        print('就地修订失败（回退新建模式）：' + str(ex), file=sys.stderr)

# ──────────── 回退：新建文档（无原文档可用时；硬编码可见修订格式） ────────────

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

d = docx.Document()
st = d.styles['Normal']
st.font.name = '宋体'
st.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)

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
T = THEMES.get(theme, THEMES['blue']).copy()
# customColors（骨架固定、皮肤开放）：可选覆盖主题配色，
# 键 bg/accent/text/sub/band/bandtext/alt，值为 6 位 hex（可带 # 前缀）；非法值忽略保底
import re as _re
custom = p.get('customColors') or {}
if isinstance(custom, dict):
    for k in ('bg', 'accent', 'text', 'sub', 'band', 'bandtext', 'alt'):
        v = custom.get(k)
        if isinstance(v, str):
            v = v.strip().lstrip('#')
            if _re.fullmatch(r'[0-9a-fA-F]{6}', v):
                T[k] = v.upper()
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
/// `stop`：/stop 令牌，在途执行可被中断；UI 直调传 None

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocExtract {
    pub path: String,
    pub text: String,
}

pub async fn doc_extract(app: AppHandle, path: Option<String>) -> CommandResult<DocExtract> {
    let path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            let handle = app.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                handle.dialog().file().blocking_pick_file()
            })
            .await
            .unwrap_or(None);
            resolve_doc_path(picked.and_then(file_path_to_string))?
        }
    };
    py_audit(
        &app,
        &format!(
            "doc_extract | path: {}",
            crate::audit::escape_for_log(&path, 300)
        ),
    );
    let input = serde_json::json!({ "path": path }).to_string();
    let r = run_doc_script(&app, "doc_extract", EXTRACT_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_extract failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("提取失败", &r.stderr));
    }
    Ok(DocExtract {
        path,
        text: r.stdout,
    })
}

pub fn resolve_doc_path(picked: Option<String>) -> CommandResult<String> {
    picked.ok_or_else(|| CommandError::Internal("用户取消了选择".into()))
}

pub fn script_fail_err(what: &str, stderr: &str) -> CommandError {
    CommandError::Internal(format!("{what}：{}", stderr.trim()))
}

pub fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
    }
}

pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
    tables: Option<serde_json::Value>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "out": out,
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_word", MAKE_DOCX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 Word 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_word | out: {out}"));
    Ok(out)
}

pub async fn doc_make_word_revisions(
    app: AppHandle,
    title: String,
    original_path: Option<String>,
    original: Vec<String>,
    revised: Vec<String>,
    filename: Option<String>,
) -> CommandResult<(String, &'static str)> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let src = original_path.clone().unwrap_or_default();
    let input = serde_json::json!({
        "title": title,
        "original_path": original_path.unwrap_or_default(),
        "original": original,
        "revised": revised,
        "out": out
    })
    .to_string();
    let (r, engine) = run_doc_revisions(
        &app,
        "doc_make_word_revisions",
        MAKE_DOCX_REVISIONS_SCRIPT,
        input,
    )
    .await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word_revisions failed | engine: {engine} | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成修订版 Word 失败", &r.stderr));
    }
    py_audit(
        &app,
        &format!("doc_make_word_revisions | engine: {engine} | src: {src} | out: {out}"),
    );
    Ok((out, engine))
}

pub async fn doc_make_excel(
    app: AppHandle,
    sheets: Vec<serde_json::Value>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "xlsx")?;
    let input = serde_json::json!({ "sheets": sheets, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_excel", MAKE_XLSX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_excel failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 Excel 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_pdf", MAKE_PDF_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_pdf failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 PDF 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    theme: Option<String>,
    custom_colors: Option<serde_json::Value>,
) -> CommandResult<String> {
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
    let input = serde_json::json!({
        "title": title,
        "slides": slides,
        "out": out,
        "theme": theme,
        "customColors": custom_colors.unwrap_or(serde_json::json!({})),
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_ppt", MAKE_PPTX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_ppt failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 PPT 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_ppt | out: {out}"));
    Ok(out)
}

pub fn strip_known_ext(name: &str, ext: &str) -> String {
    let suffix = format!(".{}", ext.to_lowercase());
    if name.to_lowercase().ends_with(&suffix) {
        let keep = name.chars().count() - suffix.chars().count();
        name.chars().take(keep).collect()
    } else {
        name.to_string()
    }
}

pub fn gen_out_path(app: &AppHandle, filename: Option<&str>, ext: &str) -> CommandResult<String> {
    gen_out_path_in(&crate::db::gen_dir(app)?, filename, ext)
}

pub fn gen_out_path_in(dir: &Path, filename: Option<&str>, ext: &str) -> CommandResult<String> {
    std::fs::create_dir_all(dir)?;
    let base = filename
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| format!("文档-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let base = strip_known_ext(&base, ext);
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(candidate.to_string_lossy().to_string())
}

// ─────────── py_exec_sync / spawn_blocking_map / run_doc_* ───────────

pub async fn spawn_blocking_map<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("执行线程异常：{e}"))?
}

pub async fn run_doc_script(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<PyRunResult, String> {
    let handle = app.clone();
    match spawn_blocking_map(move || {
        run_python(&handle, script, Some(&input), &[], Some(120), None).map_err(|e| e.to_string())
    })
    .await
    {
        Ok(inner) => Ok(inner),
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "{name} err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            Err(e.to_string())
        }
    }
}

pub async fn run_doc_revisions(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<(PyRunResult, &'static str), String> {
    let handle = app.clone();
    let name_in = name.to_string();
    match spawn_blocking_map(move || {
        let _gate = py_run_gate().lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] py::runtime::py_run_gate: {e:?}");
            e.into_inner()
        });
        if let Some(r) = run_dotnet_revisions(&handle, &input) {
            match r {
                Ok(res) if res.exit_code == Some(0) => return Ok((res, "dotnet")),
                Ok(res) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet failed, fallback python | {}",
                        crate::audit::escape_for_log(&res.stderr, 200)
                    ),
                ),
                Err(e) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet err, fallback python | {}",
                        crate::audit::escape_for_log(&e, 200)
                    ),
                ),
            }
        }
        run_python_ungated(&handle, script, Some(&input), &[], Some(120), None)
            .map(|res| (res, "python"))
            .map_err(|e| e.to_string())
    })
    .await
    {
        Ok(inner) => Ok(inner),
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "{name} err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            Err(e.to_string())
        }
    }
}

pub fn py_exec_sync(
    app: &AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, String> {
    let yolo = crate::bot::perm_mode(app) == crate::bot::PermMode::Yolo;
    let flag_on = crate::py::commands::py_get_enabled(app.clone());
    if !yolo && !flag_on {
        return Err(
            "Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」（或将授权模式切为 yolo）".into(),
        );
    }
    if yolo && !flag_on {
        py_audit(
            app,
            "py_exec | yolo_bypass | 授权模式 yolo，跳过 py-enabled 开关检查",
        );
    }
    if let Some(t) = timeout_secs {
        if t > MAX_TIMEOUT_SECS {
            py_audit(
                app,
                &format!("py_exec err | kind=timeout_cap | requested={t} cap={MAX_TIMEOUT_SECS}"),
            );
            return Err(format!("timeout 超过 {MAX_TIMEOUT_SECS}s 上限"));
        }
    }
    py_audit(
        app,
        &format!(
            "py_exec | script: {}",
            crate::audit::escape_for_log(&code, 300)
        ),
    );
    let r = match run_python(app, &code, None, &[], timeout_secs, stop) {
        Ok(r) => r,
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "py_exec err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            return Err(e.to_string());
        }
    };
    py_audit(
        app,
        &format!(
            "py_exec done | exit={:?} {}ms | out: {}",
            r.exit_code,
            r.duration_ms,
            crate::audit::escape_for_log(&r.stdout, 300)
        ),
    );
    Ok(r)
}

pub async fn py_exec_sync_async(
    app: AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<StopToken>,
) -> Result<PyRunResult, String> {
    spawn_blocking_map(move || py_exec_sync(&app, code, timeout_secs, stop.as_ref())).await
}

// 抑制 unused 警告
#[allow(dead_code)]
fn _unused_refs(
    _x: &Path,
    _y: &mut mpsc::Receiver<(&'static str, Vec<u8>, bool)>,
    _z: Duration,
    _w: Instant,
    _r: Result<PyRunResult, RunFail>,
    _e: PyEnv,
) {
}
