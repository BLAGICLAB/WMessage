//! 文档生成（Word / Excel / PDF / PPT）+ py_exec_sync 入口
//!
//! 包含 6 个固定 Python 脚本常量 + 5 个 doc_* tauri 命令 + py_exec_sync 同步入口。
//! 引擎优先 .NET OpenXML（修订版 Word），不可用回退 Python 脚本。

use std::path::Path;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::bot_slash::StopToken;
use crate::error::{CommandError, CommandResult};
use crate::py::audit::py_audit;
use crate::py::env::run_dotnet_revisions;
use crate::py::runtime::{
    py_gate_acquire, run_python, run_python_ungated, PyRunResult, MAX_TIMEOUT_SECS,
};

// 固定文档脚本模板

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

pub const MAKE_DOCX_SCRIPT: &str = r#"import json, os, re
import docx
from docx.shared import Pt, Cm, Inches
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
paras = p.get('paragraphs', [])
tables = p.get('tables', [])
images = p.get('images', [])
out = p['out']
tpl = p.get('template', '')
use_tpl = bool(tpl) and os.path.exists(tpl)
d = docx.Document(tpl) if use_tpl else docx.Document()
if use_tpl:
    # 模板锚定：继承模板的样式表/页面设置/页眉页脚；清空示例正文（保留 sectPr）
    body = d.element.body
    for child in list(body):
        if not child.tag.endswith('}sectPr'):
            body.remove(child)
else:
    style = d.styles['Normal']
    style.font.name = '宋体'
    style.font.size = Pt(12)
if use_tpl:
    # 模板模式：优先命名样式（Heading 1/Normal），版式随模板样式表走。
    # 公文/WPS 导出的模板常缺 Heading 命名样式（styleId 是数字/字母缩写），
    # add_heading 按名查样式 KeyError 会让整单失败——缺样式时降级为直接
    # 格式化（黑体+分级字号）并补 w:outlineLvl，导航窗格/目录仍按标题层级识别
    def has_style(name):
        try:
            d.styles[name]
            return True
        except KeyError:
            return False
    def add_heading_cjk(text, level):
        if has_style('Heading %d' % level):
            d.add_heading(text, level=level)
            return
        h = d.add_paragraph()
        r = h.add_run(text)
        r.font.name = '黑体'
        r.font.size = Pt({1: 16, 2: 14, 3: 12}.get(level, 12))
        # font.name 只写 w:rFonts 的 ascii/hAnsi，中文字形要显式补 eastAsia
        rpr = r._element.get_or_add_rPr()
        rpr.rFonts.set(qn('w:eastAsia'), '黑体')
        ppr = h._p.get_or_add_pPr()
        ol = OxmlElement('w:outlineLvl')
        ol.set(qn('w:val'), str(level - 1))
        ppr.append(ol)
    def add_body_para(text):
        d.add_paragraph(text)
    if title:
        add_heading_cjk(title, 1)
else:
    if title:
        h = d.add_heading('', level=1)
        r = h.add_run(title)
        r.font.name = '黑体'
        r.font.size = Pt(16)
    def add_heading_cjk(text, level):
        h = d.add_heading('', level=level)
        r = h.add_run(text)
        r.font.name = '黑体'
        # font.name 只写 w:rFonts 的 ascii/hAnsi，中文字形要显式补 eastAsia
        rpr = r._element.get_or_add_rPr()
        rpr.rFonts.set(qn('w:eastAsia'), '黑体')
    def add_body_para(text):
        pr = d.add_paragraph(text)
        pr.paragraph_format.first_line_indent = Pt(24)
for para in paras:
    m = re.match(r'^(#{1,3})\s+(.+)$', para)
    if m:
        add_heading_cjk(m.group(2), len(m.group(1)))
    elif para == '':
        d.add_paragraph('')
    else:
        add_body_para(para)
for img in images:
    if os.path.exists(img):
        d.add_picture(img, width=Inches(5.8))
for t in tables:
    rows = t.get('rows', [])
    if not rows:
        continue
    if t.get('title'):
        tp = d.add_paragraph()
        tr = tp.add_run(t['title'])
        tr.font.bold = True
    tb = d.add_table(rows=len(rows), cols=len(rows[0]))
    try:
        tb.style = 'Table Grid'
    except Exception:
        pass  # 自定义模板可能没有该内置样式
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
        ws.append(row)
        # 单元格内容来自模型输出，= 开头会被 openpyxl 存成活公式（WEBSERVICE/DDE
        # 注入面：用户打开文件即触发外链/执行提示）。翻回字符串类型：原样显示为
        # 文本不执行；数字/空值不受影响（data_type 'n'/None 不匹配 'f'）
        for c in ws[ws.max_row]:
            if c.data_type == 'f':
                c.data_type = 's'
wb.save(out)
print('已生成：' + out)
"#;

pub const MAKE_PDF_SCRIPT: &str = r#"import json, os, re
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.platypus import SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle, Image
from reportlab.lib import colors
from reportlab.lib.utils import ImageReader

pdfmetrics.registerFont(UnicodeCIDFont('STSong-Light'))
p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
paras = p.get('paragraphs', [])
tables = p.get('tables', [])
images = p.get('images', [])

# platypus 流式排版（官方推荐表格路径）：Paragraph 自动换行 + Table 网格 + 自动分页；
# CJK 断行要显式 wordWrap='CJK'，字体统一 STSong-Light CID（零字体文件依赖）
style_title = ParagraphStyle('t', fontName='STSong-Light', fontSize=18, leading=24, spaceAfter=14, wordWrap='CJK')
style_h1 = ParagraphStyle('h1', fontName='STSong-Light', fontSize=16, leading=22, spaceBefore=16, spaceAfter=8, wordWrap='CJK')
style_h2 = ParagraphStyle('h2', fontName='STSong-Light', fontSize=14, leading=20, spaceBefore=12, spaceAfter=6, wordWrap='CJK')
style_h3 = ParagraphStyle('h3', fontName='STSong-Light', fontSize=12, leading=18, spaceBefore=10, spaceAfter=4, wordWrap='CJK')
style_body = ParagraphStyle('b', fontName='STSong-Light', fontSize=11, leading=17, wordWrap='CJK')
style_cell = ParagraphStyle('c', fontName='STSong-Light', fontSize=9.5, leading=13, wordWrap='CJK')
style_tbl_title = ParagraphStyle('tt', fontName='STSong-Light', fontSize=11, leading=16, spaceBefore=10, spaceAfter=4, wordWrap='CJK')

def esc(s):
    return str(s).replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')

doc = SimpleDocTemplate(out, pagesize=A4, topMargin=20*mm, bottomMargin=18*mm, leftMargin=20*mm, rightMargin=20*mm)

# 页眉页脚：页眉 = 文档标题右对齐 + 细分隔线，页脚 = 居中页码（onPage 回调，每页绘制）
def draw_chrome(canvas, doc_):
    canvas.saveState()
    canvas.setFont('STSong-Light', 9)
    canvas.setFillColor(colors.HexColor('#6B6B6B'))
    if title:
        canvas.drawRightString(A4[0] - 20*mm, A4[1] - 12*mm, str(title)[:42])
        canvas.setStrokeColor(colors.HexColor('#D9D9D9'))
        canvas.setLineWidth(0.5)
        canvas.line(20*mm, A4[1] - 14*mm, A4[0] - 20*mm, A4[1] - 14*mm)
    canvas.drawCentredString(A4[0] / 2, 10*mm, '第 %d 页' % canvas.getPageNumber())
    canvas.restoreState()

story = []
if title:
    story.append(Paragraph(esc(title), style_title))
# 标题层级：与 create_word 同一口径——#/##/### 前缀标记一/二/三级标题
HEAD_STYLES = [style_h1, style_h2, style_h3]
for para in paras:
    m = re.match(r'^(#{1,3})\s+(.+)$', para)
    if m:
        story.append(Paragraph(esc(m.group(2)), HEAD_STYLES[len(m.group(1)) - 1]))
    elif para == '':
        story.append(Spacer(1, 8))
    else:
        story.append(Paragraph(esc(para), style_body))
# 图片：按顺序插在正文之后、表格之前；等比缩放到版心宽，高不超 180mm（超则按高反算）
for img in images:
    if not os.path.exists(img):
        continue
    iw, ih = ImageReader(img).getSize()
    if not iw or not ih:
        continue
    w = doc.width
    h = ih * w / iw
    if h > 180*mm:
        h = 180*mm
        w = iw * h / ih
    story.append(Image(img, width=w, height=h))
    story.append(Spacer(1, 6))
for t in tables:
    rows = t.get('rows', [])
    if not rows or not rows[0]:
        continue
    if t.get('title'):
        story.append(Paragraph(esc(t['title']), style_tbl_title))
    n_cols = max(len(r) for r in rows)
    data = []
    for r in rows:
        cells = [esc(str(c)) for c in r] + [''] * (n_cols - len(r))
        data.append([Paragraph(c, style_cell) for c in cells])
    # 列宽自适应：按列内容总字宽（CJK 计 2）占比分配，最小权重 4 防 0/防窄列挤压
    def disp_width(s):
        return sum(2 if ord(ch) > 0x2E80 else 1 for ch in s)
    weights = []
    for ci in range(n_cols):
        cw = max(disp_width(str(r[ci] if ci < len(r) else '')) for r in rows)
        weights.append(max(cw, 4))
    total = sum(weights)
    tb = Table(data, colWidths=[doc.width * w_ / total for w_ in weights], repeatRows=1)
    tb.setStyle(TableStyle([
        ('GRID', (0, 0), (-1, -1), 0.5, colors.grey),
        ('BACKGROUND', (0, 0), (-1, 0), colors.HexColor('#F2F4F7')),
        ('ROWBACKGROUNDS', (0, 1), (-1, -1), [colors.white, colors.HexColor('#F7F8FA')]),
        ('FONTSIZE', (0, 0), (-1, 0), 10),
        ('VALIGN', (0, 0), (-1, -1), 'TOP'),
        ('TOPPADDING', (0, 0), (-1, -1), 4),
        ('BOTTOMPADDING', (0, 0), (-1, -1), 4),
    ]))
    story.append(tb)
    story.append(Spacer(1, 10))
doc.build(story, onFirstPage=draw_chrome, onLaterPages=draw_chrome)
print('已生成：' + out)
"#;

/// N3-2：扫描版 PDF 页渲染——PyMuPDF（fitz）官方推荐渲染管线，2x zoom ≈144 DPI
///（）。exit 2 = 缺 pymupdf（Rust 侧给 pip 指引，优雅降级）；
/// 页数钳在前 max_pages 页（防大文档渲染爆内存/超时）。
pub const PDF_RENDER_SCRIPT: &str = r#"import json, os, sys
p = json.load(open('params.json', encoding='utf-8'))
try:
    import fitz
except ImportError:
    print('missing pymupdf', file=sys.stderr)
    sys.exit(2)
doc = fitz.open(p['path'])
max_pages = int(p.get('max_pages', 20))
n = min(doc.page_count, max_pages)
for i in range(n):
    pix = doc[i].get_pixmap(matrix=fitz.Matrix(2, 2))
    pix.save(os.path.join(p['out_dir'], 'page-%03d.png' % (i + 1)))
print(n)
"#;

pub const MAKE_PPTX_SCRIPT: &str = r#"import json
from pptx import Presentation
from pptx.util import Inches

p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
slides_in = p.get('slides', [])

# 版式/颜色/字体全部继承母版（明暗三明治与主题色由母版承载），本脚本零颜色代码，
# 只做占位符填充。模板路径由 Rust 侧 resolve（显式模板 → 默认标记 → 内置母版）保证可用。
prs = Presentation(p['template'])
prs.slide_width = Inches(13.333)   # 母版被外部改动时按 16:9 兜底
prs.slide_height = Inches(7.5)

# ── 版式匹配：候选名精确匹配（忽略大小写）→ 包含匹配 → 索引兜底；中英文母版都认 ──
LAYOUT_CANDIDATES = {
    'cover':   ['title slide', '标题幻灯片', '标题页'],
    'section': ['section header', '节标题', '章节页'],
    'content': ['title and content', '标题和内容', '标题与内容', '标题和文本'],
    'toc':     ['title and content', '标题和内容', '标题与内容'],
    'table':   ['title only', '仅标题'],
    'closing': ['title slide', '标题幻灯片', 'section header', '节标题'],
}

def layout_for(stype):
    names = LAYOUT_CANDIDATES.get(stype) or LAYOUT_CANDIDATES['content']
    lays = list(prs.slide_layouts)
    for want in names:
        for lay in lays:
            if (lay.name or '').strip().lower() == want:
                return lay
    for want in names:
        for lay in lays:
            if want in (lay.name or '').strip().lower():
                return lay
    return lays[0] if stype == 'cover' or len(lays) == 1 else lays[1]

def ph_by_idx(slide, idx):
    for ph in slide.placeholders:
        if ph.placeholder_format.idx == idx:
            return ph
    return None

def first_body_ph(slide):
    for ph in slide.placeholders:
        if ph.placeholder_format.idx != 0:
            return ph
    return None

def put_sub(slide, text):
    sp = ph_by_idx(slide, 1) or first_body_ph(slide)
    if sp is not None:
        sp.text = str(text)

# ── 密度硬规范：content/toc 每页要点 >5 自动拆页（不丢内容），续页标题加「（续）」──
MAX_BULLETS = 5
expanded = []
has_cover = any(s.get('type') == 'cover' for s in slides_in)
for sl in slides_in:
    st = sl.get('type', 'content')
    bullets = [str(b) for b in (sl.get('bullets') or [])]
    if st in ('content', 'toc') and len(bullets) > MAX_BULLETS:
        chunks = [bullets[i:i + MAX_BULLETS] for i in range(0, len(bullets), MAX_BULLETS)]
        for k, chunk in enumerate(chunks):
            d = dict(sl)
            d['bullets'] = chunk
            if k:
                d['title'] = (sl.get('title') or '') + '（续）'
                d['notes'] = ''
            expanded.append(d)
    else:
        expanded.append(sl)
if not has_cover:
    expanded.insert(0, {'type': 'cover', 'title': title or '演示文稿', 'subtitle': ''})

for sl in expanded:
    st = sl.get('type', 'content')
    slide = prs.slides.add_slide(layout_for(st))
    ttl = sl.get('title') or (title if st == 'cover' else '')
    if ttl:
        tp = ph_by_idx(slide, 0)
        if tp is not None:
            tp.text = str(ttl)
    if st == 'cover':
        if sl.get('subtitle'):
            put_sub(slide, sl['subtitle'])
    elif st in ('content', 'toc'):
        bullets = [str(b) for b in (sl.get('bullets') or [])]
        if st == 'toc' and not bullets:
            bullets = ['%02d  %s' % (i, s.get('title', ''))
                       for i, s in enumerate(slides_in, 1) if s.get('type') != 'cover']
        if bullets:
            bp = ph_by_idx(slide, 1) or first_body_ph(slide)
            if bp is not None:
                tf = bp.text_frame
                for i, line in enumerate(bullets):
                    para = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
                    para.text = line
    elif st == 'table':
        rows = sl.get('rows') or []
        if rows:
            nr, nc = len(rows), max(len(r) for r in rows)
            w = prs.slide_width - Inches(0.9) * 2
            h = Inches(0.35) * nr
            gfx = slide.shapes.add_table(nr, nc, Inches(0.9), Inches(1.8), w, h)
            tbl = gfx.table
            for ri, row in enumerate(rows):
                for ci in range(nc):
                    cell = tbl.cell(ri, ci)
                    cell.text = str(row[ci]) if ci < len(row) else ''
                    if ri == 0:   # 表头加粗；底纹/颜色随母版表格样式
                        for para in cell.text_frame.paragraphs:
                            for r in para.runs:
                                r.font.bold = True
    else:                         # section / closing 副标题
        if sl.get('subtitle'):
            put_sub(slide, sl['subtitle'])
    if sl.get('notes'):
        slide.notes_slide.notes_text_frame.text = str(sl['notes'])

long = [b for sl in expanded for b in (sl.get('bullets') or [])
        if len(str(b)) > 40]
if long:
    print('注意：%d 条要点超过 40 字，建议精简或拆页（规范：每条 ≤40 字）' % len(long))

prs.save(out)
print('已生成：' + out)
"#;

// 对外命令

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
            .unwrap_or_else(|e| {
                // 对话框线程异常不能伪装成「用户取消」——记审计后按未选择处理
                py_audit(
                    &app,
                    &format!(
                        "doc_extract dialog join err | {}",
                        crate::audit::escape_for_log(&e.to_string(), 200)
                    ),
                );
                None
            });
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

/// N3-2：渲染扫描版 PDF 的前 max_pages 页为 PNG（供本地 ）。
/// Err 前缀 `NEED_PYMUPDF` = 缺 pymupdf（调用方给 pip 指引，优雅降级）。
pub async fn pdf_render_pages(
    app: AppHandle,
    path: String,
    out_dir: String,
    max_pages: usize,
) -> Result<usize, String> {
    let input = serde_json::json!({
        "path": path,
        "out_dir": out_dir,
        "max_pages": max_pages
    })
    .to_string();
    let r = run_doc_script(&app, "pdf_render_pages", PDF_RENDER_SCRIPT, input).await?;
    if r.exit_code == Some(2) {
        return Err(
            "NEED_PYMUPDF: 需要 PyMuPDF 渲染扫描版 PDF：python3 -m pip install pymupdf".into(),
        );
    }
    if r.exit_code != Some(0) {
        return Err(format!("PDF 页面渲染失败：{}", r.stderr.trim()));
    }
    r.stdout
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("PDF 渲染输出异常（{e}）"))
}

pub fn script_fail_err(what: &str, stderr: &str) -> CommandError {
    CommandError::Internal(format!("{what}：{}", stderr.trim()))
}

pub fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        // 桌面端对话框可能给 file:// Url：转回本地路径；非文件 URL 返回 None（fail-closed）
        tauri_plugin_dialog::FilePath::Url(u) => u
            .to_file_path()
            .ok()
            .and_then(|pb| pb.to_str().map(String::from)),
    }
}

pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
    tables: Option<serde_json::Value>,
    images: Vec<String>,
    template: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "images": images,
        "template": template.unwrap_or_default(),
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
    // 真校验：退出码 0 ≠ 文件落盘——脚本任何「静默跳过保存」的路径都会假成功，
    // 这里对产物本身做存在性 + 非空检查（杜绝对用户谎报生成成功）
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_word 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
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
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_word_revisions 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
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
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_excel 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    tables: Option<serde_json::Value>,
    images: Vec<String>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "images": images,
        "out": out
    })
    .to_string();
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
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_pdf 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    template: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pptx")?;
    // 版式随母版：显式模板 → _default 标记 → 内置母版，永不为空
    let template = match pptx_template_resolve(&app, template.as_deref()) {
        Some(p) => p,
        None => builtin_pptx_master(&app)?,
    };
    let input = serde_json::json!({
        "title": title,
        "slides": slides,
        "out": out,
        "template": template,
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
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_ppt 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
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

// ── Word 模板管理（设置页「Word 模板」面板 + create_word 模板锚定） ──
//
// 模板 = 数据目录 word_templates/<name>.docx，用户在 Word 里排好版式上传；
// 生成时打开模板并清空正文示例（保留 sectPr 的页面设置/页眉页脚/样式表），
// 内容按命名样式填充——「LLM 填内容、模板管排版」的确定性排版口径。

fn word_templates_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("word_templates")
}

fn default_marker_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("_default")
}

/// 模板解析：显式名字优先（不存在 → None），否则默认模板（不存在 → None）。
/// None = 无模板可用，生成走内置空白默认。
pub fn word_template_resolve(app: &AppHandle, name: Option<&str>) -> Option<String> {
    word_template_resolve_in(&word_templates_dir(app), name)
}

fn word_template_resolve_in(dir: &std::path::Path, name: Option<&str>) -> Option<String> {
    let explicit = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .map(|n| dir.join(format!("{n}.docx")));
    let candidate = match explicit {
        Some(p) => p,
        None => {
            let def = std::fs::read_to_string(default_marker_path(dir)).ok()?;
            dir.join(format!("{def}.docx"))
        }
    };
    candidate
        .exists()
        .then(|| candidate.to_string_lossy().to_string())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WordTemplateInfo {
    pub name: String,
    pub mtime_ms: i64,
    pub is_default: bool,
}

#[tauri::command]
pub fn word_template_list(app: AppHandle) -> CommandResult<Vec<WordTemplateInfo>> {
    let dir = word_templates_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let def = std::fs::read_to_string(default_marker_path(&dir)).unwrap_or_default();
    let mut out: Vec<WordTemplateInfo> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| CommandError::IoError(e.to_string()))? {
        let entry = entry.map_err(|e| CommandError::IoError(e.to_string()))?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("docx") {
            continue;
        }
        let Some(name) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let mtime_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(WordTemplateInfo {
            name: name.to_string(),
            mtime_ms,
            is_default: def == name,
        });
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.mtime_ms));
    Ok(out)
}

#[tauri::command]
pub fn word_template_import(app: AppHandle, path: String) -> CommandResult<String> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "请选择一个 .docx 模板文件".into(),
        });
    }
    if src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.eq_ignore_ascii_case("docx"))
        .unwrap_or(true)
    {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "模板必须是 .docx 文件".into(),
        });
    }
    let dir = word_templates_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest = dir.join(format!("{name}.docx"));
    if dest.exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name.clone(),
            reason: "同名模板已存在：请先删除再上传".into(),
        });
    }
    std::fs::copy(&src, &dest).map_err(|e| CommandError::IoError(e.to_string()))?;
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_import | {}",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(name)
}

#[tauri::command]
pub fn word_template_delete(app: AppHandle, name: String) -> CommandResult<()> {
    let dir = word_templates_dir(&app);
    let p = dir.join(format!("{name}.docx"));
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    if std::fs::read_to_string(default_marker_path(&dir))
        .map(|d| d == name)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(default_marker_path(&dir));
    }
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_delete | {}",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(())
}

#[tauri::command]
pub fn word_template_set_default(app: AppHandle, name: String) -> CommandResult<()> {
    let dir = word_templates_dir(&app);
    let p = dir.join(format!("{name}.docx"));
    if !p.exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "模板不存在".into(),
        });
    }
    std::fs::write(default_marker_path(&dir), &name)
        .map_err(|e| CommandError::IoError(e.to_string()))?;
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_default | {}",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(())
}

// ── PPT 模板（create_ppt 版式随母版；无用户模板时回退内置母版）──

/// 内置母版（午夜商务主题，16:9，封面/章节版式深底白字）：python-pptx 默认模板
/// 改 theme 配色/字体（含 eastAsia 雅黑）+ 版式深底生成，随二进制分发
const BUILTIN_PPTX_MASTER: &[u8] = include_bytes!("../../assets/pptx-master/default.pptx");

fn pptx_templates_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("pptx_templates")
}

/// 内置母版缓存路径（app 拥有的缓存：字节与内嵌资产不一致即覆盖，无删除语义；
/// 用户自传模板在 pptx_templates/，两套目录互不干扰）
fn builtin_pptx_master(app: &AppHandle) -> CommandResult<String> {
    let dir = crate::db::data_dir(app).join("builtin_templates");
    builtin_pptx_master_in(&dir)
}

fn builtin_pptx_master_in(dir: &std::path::Path) -> CommandResult<String> {
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let dest = dir.join("pptx-default.pptx");
    let stale = std::fs::read(&dest)
        .map(|bytes| bytes != BUILTIN_PPTX_MASTER)
        .unwrap_or(true);
    if stale {
        std::fs::write(&dest, BUILTIN_PPTX_MASTER)
            .map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    Ok(dest.to_string_lossy().to_string())
}

/// 模板解析：显式名优先（不存在 → None），否则默认模板（不存在 → None）。
/// None = 无用户模板可用，调用方回退内置母版。
pub fn pptx_template_resolve(app: &AppHandle, name: Option<&str>) -> Option<String> {
    pptx_template_resolve_in(&pptx_templates_dir(app), name)
}

fn pptx_template_resolve_in(dir: &std::path::Path, name: Option<&str>) -> Option<String> {
    let explicit = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .map(|n| dir.join(format!("{n}.pptx")));
    let candidate = match explicit {
        Some(p) => p,
        None => {
            let def = std::fs::read_to_string(default_marker_path(dir)).ok()?;
            dir.join(format!("{def}.pptx"))
        }
    };
    candidate
        .exists()
        .then(|| candidate.to_string_lossy().to_string())
}

// py_exec_sync / spawn_blocking_map / run_doc_*

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
        // doc 生成流持一个许可跨 dotnet+python 兜底（退出态在此拒发）
        let _gate = py_gate_acquire().map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod word_template_tests {
    use super::*;

    #[test]
    fn resolve_prefers_explicit_then_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("a.docx"), b"x").unwrap();
        std::fs::write(dir.join("b.docx"), b"x").unwrap();
        std::fs::write(default_marker_path(dir), "b").unwrap();

        // 显式名命中（存在才返回）
        assert!(word_template_resolve_in(dir, Some("a")).is_some());
        // 显式名不存在 → None（不回退默认，用户点名了就不能拿别的顶）
        assert!(word_template_resolve_in(dir, Some("nope")).is_none());
        // 未指定 → 默认
        assert_eq!(
            word_template_resolve_in(dir, None).unwrap(),
            dir.join("b.docx").to_string_lossy().to_string()
        );
        // 默认文件被删 → None
        std::fs::remove_file(dir.join("b.docx")).unwrap();
        assert!(word_template_resolve_in(dir, None).is_none());
    }

    #[test]
    fn resolve_without_marker_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(word_template_resolve_in(tmp.path(), None).is_none());
    }
}

#[cfg(test)]
mod pptx_template_tests {
    use super::*;

    #[test]
    fn resolve_prefers_explicit_then_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("a.pptx"), b"x").unwrap();
        std::fs::write(dir.join("b.pptx"), b"x").unwrap();
        std::fs::write(default_marker_path(dir), "b").unwrap();

        assert!(pptx_template_resolve_in(dir, Some("a")).is_some());
        // 显式名不存在 → None（不回退默认，调用方拿内置母版顶）
        assert!(pptx_template_resolve_in(dir, Some("nope")).is_none());
        assert_eq!(
            pptx_template_resolve_in(dir, None).unwrap(),
            dir.join("b.pptx").to_string_lossy().to_string()
        );
        std::fs::remove_file(dir.join("b.pptx")).unwrap();
        assert!(pptx_template_resolve_in(dir, None).is_none());
    }

    #[test]
    fn builtin_master_materializes_idempotent_and_self_heals() {
        let tmp = tempfile::tempdir().unwrap();
        // 空资产不合法：内嵌母版必须是 zip（pptx = zip 容器，PK 魔数）
        assert!(BUILTIN_PPTX_MASTER.len() > 1000);
        assert_eq!(&BUILTIN_PPTX_MASTER[..2], b"PK");

        let first = builtin_pptx_master_in(tmp.path()).unwrap();
        let written = std::fs::read(&first).unwrap();
        assert_eq!(written, BUILTIN_PPTX_MASTER);
        // 幂等：二次调用字节不变不再写（mtime 不变）
        let meta1 = std::fs::metadata(&first).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let again = builtin_pptx_master_in(tmp.path()).unwrap();
        assert_eq!(again, first);
        let meta2 = std::fs::metadata(&again).unwrap().modified().unwrap();
        assert_eq!(meta1, meta2, "字节一致时不得重写");
        // 自愈：缓存损坏后重写
        std::fs::write(&first, b"corrupted").unwrap();
        builtin_pptx_master_in(tmp.path()).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), BUILTIN_PPTX_MASTER);
    }

    #[test]
    fn script_fills_placeholders_without_theme_code() {
        // 回归锁：v2 脚本走母版占位符填充，主题/自定义配色代码不得回流
        assert!(MAKE_PPTX_SCRIPT.contains("Presentation(p['template'])"));
        assert!(!MAKE_PPTX_SCRIPT.contains("THEMES"));
        assert!(!MAKE_PPTX_SCRIPT.contains("customColors"));
        assert!(
            MAKE_PPTX_SCRIPT.contains("MAX_BULLETS = 5"),
            "密度拆页闸必须在"
        );
    }
}

// 回归锁（读源字符串，同 bot_py 对 runtime.rs 的锁法）：
// 五个生成函数都必须带「退出码 0 ≠ 文件落盘」真校验——脚本静默跳过保存的
// 路径会假成功（杜绝对用户谎报已生成）。新增 doc_make_* 时必须同步补齐块，
// 否则此测试红。
#[cfg(test)]
mod fake_success_guard_tests {
    #[test]
    fn fake_success_guard_covers_all_doc_makers() {
        let src = include_str!("document.rs");
        for name in [
            "doc_make_word",
            "doc_make_word_revisions",
            "doc_make_excel",
            "doc_make_pdf",
            "doc_make_ppt",
        ] {
            let marker = format!("{name} 假成功拦截");
            assert!(src.contains(&marker), "{name} 缺少假成功拦截块");
        }
    }

    /// MAKE_DOCX_SCRIPT 的 Heading 样式守卫回归锁：公文/WPS 导出的模板常无
    /// Heading 命名样式，add_heading 按名查样式 KeyError 会让整单失败（用户
    /// 真实模板触发过）。脚本必须有样式存在性守卫 + outlineLvl 降级
    #[test]
    fn docx_script_guards_missing_heading_styles() {
        let src = include_str!("document.rs");
        assert!(src.contains("def has_style(name)"), "缺样式守卫被移除");
        assert!(src.contains("w:outlineLvl"), "outlineLvl 降级被移除");
        assert!(
            src.contains("if has_style('Heading %d' % level)"),
            "add_heading 未走样式守卫"
        );
    }
}
