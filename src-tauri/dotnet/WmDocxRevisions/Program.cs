// wm-docx-revisions: 修订模式（track changes）Word 生成器（OpenXML SDK 官方修订标记）
// 用法: wm-docx-revisions <params.json>
// params: { title, original_path, original[], revised[], out }
//
// 2026-09-02 老板拍板：修订模式改为「原文档副本就地打修订标记」——保留原文的格式、
// 字体、样式、表格结构（原先从零新建宋体文档，格式全丢）。
// original_path 可读时：复制原文 → 段落级对齐（与 extract_document 同一口径：正文非空段落
// 文档序在前、表格行在后）→ 相同段落原样不动（格式自然保留），改动段落做行内字符级 diff
// （未变片段沿用原 run 的 rPr，删除/新增片段克隆锚点 run 的 rPr）。
// original_path 缺失/不可读时回退旧行为：用模型传的 original 行列表新建宋体文档。
//
// 修订标记（对齐 minimax-docx/references/track_changes_guide.md 铁律）：
// - w:ins 内用 w:t（InsertedRun + Run(Text)）
// - w:del 内用 w:delText（DeletedRun + Run(DeletedText)），用 w:t 会静默损坏文档
// - 每个修订带唯一递增 w:id + author（2026-09-02 老板拍板：不写 w:date 修订日期）
// 官方修订的视觉标记（删除线/下划线/颜色）由 Word 审阅视图渲染，不写死字符级格式。

using System.Text.Json;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Wordprocessing;

const string Author = "WMessage AI";

// ───────────────────────── 参数 ─────────────────────────

if (args.Length < 1)
{
    Console.Error.WriteLine("usage: wm-docx-revisions <params.json>");
    return 2;
}

var p = JsonDocument.Parse(await File.ReadAllTextAsync(args[0])).RootElement;
string title = p.TryGetProperty("title", out var t) ? t.GetString() ?? "" : "";
string originalPath = p.TryGetProperty("original_path", out var op) ? op.GetString() ?? "" : "";
var origModel = ReadStringList(p, "original");
var revised = ReadStringList(p, "revised");
string outPath = p.GetProperty("out").GetString()!;

if (revised.Count == 0)
{
    Console.Error.WriteLine("revised 不能为空");
    return 1;
}

int revId = 1000;
int NextId() => ++revId;

// ───────────────────────── 就地修订（保留原文格式）─────────────────────────

bool canInPlace = !string.IsNullOrEmpty(originalPath)
    && File.Exists(originalPath)
    && originalPath.EndsWith(".docx", StringComparison.OrdinalIgnoreCase);
if (canInPlace)
{
    try
    {
        File.Copy(originalPath, outPath, overwrite: true);
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"复制原文失败（回退新建模式）：{ex.Message}");
        canInPlace = false;
    }
}
if (canInPlace)
{
    try
    {
        ReviseInPlace(outPath, revised, NextId);
        Console.WriteLine($"已生成（修订模式·保留原文格式，原文取自文件）：{outPath}");
        return 0;
    }
    catch (Exception ex)
    {
        // 就地失败（文件损坏/结构异常）不硬挂：回退新建模式，保证有产物
        Console.Error.WriteLine($"就地修订失败（回退新建模式）：{ex.Message}");
    }
}

// ───────────────────────── 回退：新建文档（无原文档可用时）─────────────────────────

// 原文：优先从文件回读；文件行数多于模型所见（提取被截断）时用模型传的列表，保证对比范围一致
var origFile = new List<string>();
if (!string.IsNullOrEmpty(originalPath) && File.Exists(originalPath)
    && originalPath.EndsWith(".docx", StringComparison.OrdinalIgnoreCase))
{
    origFile = ReadDocxLines(originalPath);
}
List<string> orig;
string srcNote;
if (origFile.Count > 0 && (origModel.Count == 0 || origFile.Count <= origModel.Count))
{
    orig = origFile;
    srcNote = "原文取自文件";
}
else if (origModel.Count > 0)
{
    orig = origModel;
    srcNote = "原文取自提取列表";
}
else
{
    Console.Error.WriteLine("缺少原文：请提供 original_path（Word 路径）或 original（原文行列表）");
    return 1;
}

using (var freshDoc = WordprocessingDocument.Create(outPath, WordprocessingDocumentType.Document))
{
    var mainPart = freshDoc.AddMainDocumentPart();
    mainPart.Document = new Document(new Body());
    var body = mainPart.Document.Body!;

    // 正文默认字体：宋体 12pt（sz 24 半点）
    var stylesPart = mainPart.AddNewPart<StyleDefinitionsPart>();
    stylesPart.Styles = new Styles(
        new Style(
            new StyleName { Val = "Normal" },
            new StyleRunProperties(
                new RunFonts { Ascii = "宋体", HighAnsi = "宋体", EastAsia = "宋体" },
                new FontSize { Val = "24" },
                new FontSizeComplexScript { Val = "24" }))
        { Type = StyleValues.Paragraph, StyleId = "Normal", Default = true });
    stylesPart.Styles.Save();

    if (!string.IsNullOrEmpty(title))
    {
        // 标题字体：黑体 16pt
        var hr = new Run(new Text(title) { Space = SpaceProcessingModeValues.Preserve });
        hr.RunProperties = new RunProperties(
            new RunFonts { Ascii = "黑体", HighAnsi = "黑体", EastAsia = "黑体" },
            new FontSize { Val = "32" },
            new FontSizeComplexScript { Val = "32" });
        var h = new Paragraph(hr);
        h.ParagraphProperties = new ParagraphProperties(
            new ParagraphStyleId { Val = "Heading1" },
            new Justification { Val = JustificationValues.Left });
        body.Append(h);
    }

    void FreshIns(Paragraph para, string text) => para.Append(new InsertedRun(
        new Run(new Text(text) { Space = SpaceProcessingModeValues.Preserve }))
        { Id = NextId().ToString(), Author = Author });

    void FreshDel(Paragraph para, string text) => para.Append(new DeletedRun(
        new Run(new DeletedText(text) { Space = SpaceProcessingModeValues.Preserve }))
        { Id = NextId().ToString(), Author = Author });

    Run FreshPlain(string text) => new(
        new RunProperties(new RunFonts { Ascii = "宋体", HighAnsi = "宋体", EastAsia = "宋体" }),
        new Text(text) { Space = SpaceProcessingModeValues.Preserve });

    // 字符级 diff 进段落（replace 段落的行内对齐）
    void FreshDiffInto(Paragraph para, string a, string b)
    {
        foreach (var op in DiffText(a, b))
        {
            switch (op.Tag)
            {
                case "equal": para.Append(FreshPlain(a.Substring(op.A0, op.A1 - op.A0))); break;
                case "delete": FreshDel(para, a.Substring(op.A0, op.A1 - op.A0)); break;
                case "insert": FreshIns(para, b.Substring(op.B0, op.B1 - op.B0)); break;
                default: // replace
                    FreshDel(para, a.Substring(op.A0, op.A1 - op.A0));
                    FreshIns(para, b.Substring(op.B0, op.B1 - op.B0));
                    break;
            }
        }
    }

    // 段落级对齐后逐段生成（对齐 Python 版语义：equal 原文直出，delete 整段标删，
    // insert 整段标增，replace 按行配对做行内字符级 diff，多出部分整段标删/增）
    foreach (var seg in DiffList(orig, revised))
    {
        switch (seg.Tag)
        {
            case "equal":
                for (int i = seg.A0; i < seg.A1; i++)
                {
                    var para = body.AppendChild(new Paragraph());
                    if (orig[i].Length > 0) para.Append(FreshPlain(orig[i]));
                }
                break;
            case "delete":
                for (int i = seg.A0; i < seg.A1; i++)
                {
                    var para = body.AppendChild(new Paragraph());
                    if (orig[i].Length > 0) FreshDel(para, orig[i]);
                }
                break;
            case "insert":
                for (int j = seg.B0; j < seg.B1; j++)
                {
                    var para = body.AppendChild(new Paragraph());
                    if (revised[j].Length > 0) FreshIns(para, revised[j]);
                }
                break;
            default: // replace
            {
                int n = Math.Min(seg.A1 - seg.A0, seg.B1 - seg.B0);
                for (int k = 0; k < n; k++)
                {
                    var para = body.AppendChild(new Paragraph());
                    FreshDiffInto(para, orig[seg.A0 + k], revised[seg.B0 + k]);
                }
                for (int i = seg.A0 + n; i < seg.A1; i++)
                {
                    var para = body.AppendChild(new Paragraph());
                    if (orig[i].Length > 0) FreshDel(para, orig[i]);
                }
                for (int j = seg.B0 + n; j < seg.B1; j++)
                {
                    var para = body.AppendChild(new Paragraph());
                    if (revised[j].Length > 0) FreshIns(para, revised[j]);
                }
                break;
            }
        }
    }

    mainPart.Document.Save();
}
Console.WriteLine($"已生成（修订模式，{srcNote}）：{outPath}");
return 0;

// ───────────────────────── 就地修订实现 ─────────────────────────

// 对齐单元：正文段落（ParaUnit）或表格行（RowUnit），声明见文件末尾
// （提取口径：正文非空段落文档序在前，表格行在后）

void ReviseInPlace(string path, List<string> revisedLines, Func<int> nextId)
{
    using var doc = WordprocessingDocument.Open(path, true);
    var body = doc.MainDocumentPart!.Document!.Body!;

    var units = new List<Unit>();
    foreach (var para in body.Elements<Paragraph>())
    {
        var text = para.InnerText;
        if (!string.IsNullOrWhiteSpace(text)) units.Add(new ParaUnit(para, text));
    }
    foreach (var tbl in body.Elements<Table>())
        foreach (var row in tbl.Elements<TableRow>())
            units.Add(new RowUnit(row, RowText(row)));

    var origLines = units.Select(u => u.Text).ToList();

    // 锚点：新增段落插到最近一个已处理单元之后（行单元则插到所属表格之后）
    OpenXmlElement AnchorOf(int unitIndex)
    {
        if (unitIndex < 0) return null!;
        return units[unitIndex] switch
        {
            ParaUnit pu => pu.Para,
            RowUnit ru => (OpenXmlElement)ru.Row.Ancestors<Table>().First(),
            _ => null!,
        };
    }

    foreach (var seg in DiffList(origLines, revisedLines))
    {
        switch (seg.Tag)
        {
            case "equal":
                break; // 原样不动：格式自然保留
            case "delete":
                for (int i = seg.A0; i < seg.A1; i++) DeleteUnit(units[i], nextId);
                break;
            case "insert":
            {
                OpenXmlElement? anchor = seg.A0 > 0 ? AnchorOf(seg.A0 - 1) : null;
                for (int j = seg.B0; j < seg.B1; j++)
                {
                    if (revisedLines[j].Length == 0) continue;
                    anchor = InsertRevisedParagraph(body, anchor, revisedLines[j], nextId);
                }
                break;
            }
            default: // replace
            {
                int n = Math.Min(seg.A1 - seg.A0, seg.B1 - seg.B0);
                OpenXmlElement? anchor = seg.A0 > 0 ? AnchorOf(seg.A0 - 1) : null;
                for (int k = 0; k < n; k++)
                {
                    anchor = ReplaceUnit(body, units[seg.A0 + k], revisedLines[seg.B0 + k], nextId);
                }
                for (int i = seg.A0 + n; i < seg.A1; i++)
                {
                    DeleteUnit(units[i], nextId);
                    anchor = AnchorOf(i);
                }
                for (int j = seg.B0 + n; j < seg.B1; j++)
                {
                    if (revisedLines[j].Length == 0) continue;
                    anchor = InsertRevisedParagraph(body, anchor, revisedLines[j], nextId);
                }
                break;
            }
        }
    }
    doc.MainDocumentPart!.Document.Save();
}

// 表格行文本：单元格 " | " 连接（与 extract_document 同一口径）
static string RowText(TableRow row) =>
    string.Join(" | ", row.Elements<TableCell>().Select(c => c.InnerText.Trim()));

// 整单元标删：段落内全部文本 run 转 w:del（保留各 run 原 rPr），行则逐单元格处理
static void DeleteUnit(Unit unit, Func<int> nextId)
{
    switch (unit)
    {
        case ParaUnit pu:
            MarkParagraphDeleted(pu.Para, nextId);
            break;
        case RowUnit ru:
            foreach (var cell in ru.Row.Elements<TableCell>())
                foreach (var para in cell.Elements<Paragraph>())
                    MarkParagraphDeleted(para, nextId);
            break;
    }
}

// 替换单元：段落做行内字符级 diff；表格行按 " | " 拆回单元格逐格 diff，
// 格数对不上时整行标删 + 表后插新段落（保底不丢内容）。
// 返回后续插入操作的锚点元素。
static OpenXmlElement ReplaceUnit(Body body, Unit unit, string newText, Func<int> nextId)
{
    if (unit is ParaUnit pu)
    {
        ReviseParagraph(pu.Para, pu.Text, newText, nextId);
        return pu.Para;
    }
    var ru = (RowUnit)unit;
    var tbl = (OpenXmlElement)ru.Row.Ancestors<Table>().First();
    var cells = ru.Row.Elements<TableCell>().ToList();
    var parts = newText.Split(" | ");
    if (parts.Length == cells.Count)
    {
        for (int i = 0; i < cells.Count; i++)
        {
            var paras = cells[i].Elements<Paragraph>().ToList();
            var firstTextPara = paras.FirstOrDefault(x => !string.IsNullOrWhiteSpace(x.InnerText));
            if (firstTextPara != null)
            {
                ReviseParagraph(firstTextPara, firstTextPara.InnerText, parts[i].Trim(), nextId);
                foreach (var extra in paras.Where(x => x != firstTextPara && !string.IsNullOrWhiteSpace(x.InnerText)))
                    MarkParagraphDeleted(extra, nextId);
            }
            else if (paras.Count > 0)
            {
                // 空单元格：首段直接插新增
                paras[0].Append(MakeIns(parts[i].Trim(), null, nextId));
            }
        }
        return tbl;
    }
    DeleteUnit(ru, nextId);
    return InsertRevisedParagraph(body, tbl, newText, nextId);
}

// 整段标删：每个含文本的 run 转 DeletedRun（克隆 rPr，w:t → w:delText），
// 无文本的 run（图片/对象等）保持原样不动
static void MarkParagraphDeleted(Paragraph para, Func<int> nextId)
{
    var runs = para.Elements<Run>().ToList();
    var dels = new List<DeletedRun>();
    foreach (var r in runs)
    {
        var text = string.Concat(r.Elements<Text>().Select(x => x.Text));
        if (text.Length == 0) continue;
        dels.Add(MakeDel(text, r.RunProperties, nextId));
    }
    foreach (var r in runs)
    {
        if (r.Elements<Text>().Any()) r.Remove();
    }
    foreach (var d in dels) para.Append(d);
}

// 行内字符级 diff：equal 片段沿用原 run（克隆 rPr 拆段），del/ins 克隆锚点 rPr。
// 段落里若有超链接等非常规结构导致 run 文本拼接 ≠ 段落文本，保底整段删+整段增。
static void ReviseParagraph(Paragraph para, string oldText, string newText, Func<int> nextId)
{
    var runs = para.Elements<Run>().ToList();
    var map = new List<(int s, int e, Run r)>();
    int pos = 0;
    foreach (var r in runs)
    {
        var t = string.Concat(r.Elements<Text>().Select(x => x.Text));
        map.Add((pos, pos + t.Length, r));
        pos += t.Length;
    }
    if (pos != oldText.Length || string.Concat(map.Select(m => RunText(m.r))) != oldText)
    {
        // 保底：整段删（保留各 run rPr）+ 整段增（锚点 rPr）
        var anchorRpr = map.Count > 0 ? map[^1].r.RunProperties : null;
        MarkParagraphDeleted(para, nextId);
        if (newText.Length > 0) para.Append(MakeIns(newText, anchorRpr, nextId));
        return;
    }

    RunProperties? RprAt(int i)
    {
        foreach (var m in map)
            if (i >= m.s && i < m.e) return m.r.RunProperties;
        return map.Count > 0 ? map[^1].r.RunProperties : null;
    }

    // [a,b) 区间按 run 边界拆片，每片带原 rPr
    IEnumerable<(string piece, RunProperties? rpr)> Pieces(int a, int b)
    {
        foreach (var m in map)
        {
            int lo = Math.Max(a, m.s), hi = Math.Min(b, m.e);
            if (lo < hi) yield return (RunText(m.r).Substring(lo - m.s, hi - lo), m.r.RunProperties);
        }
    }

    var content = new List<OpenXmlElement>();
    foreach (var op in DiffText(oldText, newText))
    {
        switch (op.Tag)
        {
            case "equal":
                foreach (var (piece, rpr) in Pieces(op.A0, op.A1))
                    content.Add(MakePlain(piece, rpr));
                break;
            case "delete":
                foreach (var (piece, rpr) in Pieces(op.A0, op.A1))
                    content.Add(MakeDel(piece, rpr, nextId));
                break;
            case "insert":
                content.Add(MakeIns(newText.Substring(op.B0, op.B1 - op.B0), RprAt(op.A0), nextId));
                break;
            default: // replace
                foreach (var (piece, rpr) in Pieces(op.A0, op.A1))
                    content.Add(MakeDel(piece, rpr, nextId));
                content.Add(MakeIns(newText.Substring(op.B0, op.B1 - op.B0), RprAt(op.A0), nextId));
                break;
        }
    }
    foreach (var r in runs) r.Remove();
    foreach (var el in content) para.Append(el);
}

// 表后/段后插入新段落（整段 w:ins）：pPr 克隆自锚点段落继承样式，
// rPr 克隆自锚点段落首个含文本 run 继承字体；无锚点则插到正文开头（sectPr 之前）
static Paragraph InsertRevisedParagraph(Body body, OpenXmlElement? anchor, string text, Func<int> nextId)
{
    var para = new Paragraph();
    var anchorPara = anchor as Paragraph ?? anchor?.Ancestors<Paragraph>().FirstOrDefault();
    if (anchorPara?.ParagraphProperties != null)
        para.Append(anchorPara.ParagraphProperties.CloneNode(true));
    var rpr = anchorPara?.Elements<Run>()
        .FirstOrDefault(r => r.Elements<Text>().Any())?.RunProperties;
    para.Append(MakeIns(text, rpr, nextId));
    if (anchor == null)
    {
        var sect = body.Elements<SectionProperties>().FirstOrDefault();
        if (sect != null) body.InsertBefore(para, sect);
        else body.Append(para);
    }
    else
    {
        anchor.InsertAfterSelf(para);
    }
    return para;
}

// ───────────────────────── 修订元素构造 ─────────────────────────

static string RunText(Run r) => string.Concat(r.Elements<Text>().Select(x => x.Text));

// rPr 必须是 run 的第一个子元素：统一 PrependChild 保证顺序，空 rPr 不加
static Run RunWithRpr(RunProperties? rpr, OpenXmlElement textEl)
{
    var r = new Run(textEl);
    if (rpr != null) r.PrependChild(rpr.CloneNode(true));
    return r;
}

static Run MakePlain(string text, RunProperties? rpr) =>
    RunWithRpr(rpr, new Text(text) { Space = SpaceProcessingModeValues.Preserve });

static InsertedRun MakeIns(string text, RunProperties? rpr, Func<int> nextId) => new(
    RunWithRpr(rpr, new Text(text) { Space = SpaceProcessingModeValues.Preserve }))
    { Id = nextId().ToString(), Author = Author };

static DeletedRun MakeDel(string text, RunProperties? rpr, Func<int> nextId) => new(
    RunWithRpr(rpr, new DeletedText(text) { Space = SpaceProcessingModeValues.Preserve }))
    { Id = nextId().ToString(), Author = Author };

// ───────────────────────── 工具函数 ─────────────────────────

static List<string> ReadStringList(JsonElement p, string name)
{
    var list = new List<string>();
    if (p.TryGetProperty(name, out var arr) && arr.ValueKind == JsonValueKind.Array)
        foreach (var item in arr.EnumerateArray())
            if (item.ValueKind == JsonValueKind.String)
                list.Add(item.GetString()!);
    return list;
}

// 回读原文 docx：段落文本 + 表格行（与 Python 提取脚本同一口径）
static List<string> ReadDocxLines(string path)
{
    var lines = new List<string>();
    using var d = WordprocessingDocument.Open(path, false);
    var body = d.MainDocumentPart?.Document?.Body;
    if (body == null) return lines;
    foreach (var el in body.ChildElements)
    {
        if (el is Paragraph para)
        {
            var text = para.InnerText;
            if (!string.IsNullOrWhiteSpace(text)) lines.Add(text);
        }
        else if (el is Table tbl)
        {
            foreach (var row in tbl.Elements<TableRow>())
            {
                var cells = row.Elements<TableCell>()
                    .Select(c => c.InnerText.Trim())
                    .ToList();
                lines.Add(string.Join(" | ", cells));
            }
        }
    }
    return lines;
}

// ───────────────────────── diff（LCS opcodes，对齐 difflib.SequenceMatcher 语义） ──

// 最长公共子序列 → opcodes（equal/delete/insert/replace 合并相邻段）。
// 对 string[]（段落级）和 string（字符级）共用：统一按元素比较。
static List<Opcode> DiffList<T>(IReadOnlyList<T> a, IReadOnlyList<T> b) where T : IEquatable<T>
{
    int n = a.Count, m = b.Count;
    // LCS 长度表（从右下角往回递推）
    var dp = new int[n + 1, m + 1];
    for (int i = n - 1; i >= 0; i--)
        for (int j = m - 1; j >= 0; j--)
            dp[i, j] = a[i].Equals(b[j]) ? dp[i + 1, j + 1] + 1 : Math.Max(dp[i + 1, j], dp[i, j + 1]);

    // 回溯生成 raw opcodes
    var raw = new List<Opcode>();
    int x = 0, y = 0;
    while (x < n && y < m)
    {
        if (a[x].Equals(b[y])) { raw.Add(new Opcode("equal", x, x + 1, y, y + 1)); x++; y++; }
        else if (dp[x + 1, y] >= dp[x, y + 1]) { raw.Add(new Opcode("delete", x, x + 1, y, y)); x++; }
        else { raw.Add(new Opcode("insert", x, x, y, y + 1)); y++; }
    }
    while (x < n) { raw.Add(new Opcode("delete", x, x + 1, y, y)); x++; }
    while (y < m) { raw.Add(new Opcode("insert", x, x, y, y + 1)); y++; }

    // 合并相邻同 tag；delete+insert 相邻合并为 replace（对齐 difflib 输出形态）
    var merged = new List<Opcode>();
    foreach (var op in raw)
    {
        if (merged.Count > 0)
        {
            var last = merged[^1];
            if (last.Tag == op.Tag)
            {
                merged[^1] = last with { A1 = op.A1, B1 = op.B1 };
                continue;
            }
            // delete 紧跟 insert（或反向）→ 合并成 replace
            if ((last.Tag == "delete" && op.Tag == "insert") ||
                (last.Tag == "insert" && op.Tag == "delete"))
            {
                merged[^1] = new Opcode("replace",
                    Math.Min(last.A0, op.A0), Math.Max(last.A1, op.A1),
                    Math.Min(last.B0, op.B0), Math.Max(last.B1, op.B1));
                continue;
            }
        }
        merged.Add(op);
    }
    return merged;
}

// 字符串重载：按 char 切片。LCS 是 O(n×m) 内存，超长段落退化为整段替换
// （审阅可读性优先：100万字节的 dp 表不划算，整段删+增语义等价）
static List<Opcode> DiffText(string a, string b)
{
    const int MaxCells = 4_000_000; // 4M int ≈ 16MB，超出则整段 replace
    if ((long)a.Length * b.Length > MaxCells)
        return new List<Opcode> { new Opcode("replace", 0, a.Length, 0, b.Length) };
    return DiffList(a.ToCharArray(), b.ToCharArray());
}

// ───────────────────────── 类型声明（顶级语句之后） ─────────────────────────

// 对齐单元：正文段落或表格行（与 extract_document 同一口径）
abstract record Unit(string Text);
sealed record ParaUnit(Paragraph Para, string Text) : Unit(Text);
sealed record RowUnit(TableRow Row, string Text) : Unit(Text);

readonly record struct Opcode(string Tag, int A0, int A1, int B0, int B1);
