import json, os, sys, difflib, shutil, copy
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
    print('revised 不能为空', file=sys.stderr); sys.exit(1)

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
    print('缺少原文：请提供 original_path（Word 路径）或 original（原文行列表）', file=sys.stderr); sys.exit(1)

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
