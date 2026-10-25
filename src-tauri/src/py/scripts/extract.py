import json, sys, os
p = json.load(open('params.json', encoding='utf-8'))['path']
ext = os.path.splitext(p)[1].lower()
if not os.path.exists(p):
    print('文件不存在：' + p, file=sys.stderr); sys.exit(1)
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
    print('不支持的格式：' + ext, file=sys.stderr); sys.exit(1)
