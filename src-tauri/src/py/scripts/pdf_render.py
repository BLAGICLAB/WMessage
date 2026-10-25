import json, os, sys
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
