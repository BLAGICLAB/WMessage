import json, os, re
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
