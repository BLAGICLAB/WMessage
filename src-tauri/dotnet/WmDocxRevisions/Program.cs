// wm-docx-revisions: 修订模式（track changes）Word 生成器（OpenXML SDK 官方修订标记）
// 用法: wm-docx-revisions <params.json>
// params: { title, original_path, original[], revised[], out }
// 原文优先回读 original_path 文件（保真）；读不到/更长时用模型传的 original 行列表。
//
// 修订标记（对齐 minimax-docx/references/track_changes_guide.md 铁律）：
// - w:ins 内用 w:t（InsertedRun + Run(Text)）
// - w:del 内用 w:delText（DeletedRun + Run(DeletedText)），用 w:t 会静默损坏文档
// - 每个修订带唯一递增 w:id + author + ISO8601 UTC date
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

// ───────────────────────── 生成文档 ─────────────────────────

int revId = 1000;
int NextId() => ++revId;
var date = DateTime.UtcNow;

using var doc = WordprocessingDocument.Create(outPath, WordprocessingDocumentType.Document);
var mainPart = doc.AddMainDocumentPart();
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

Run PlainRun(string text) => new(
    new RunProperties(new RunFonts { Ascii = "宋体", HighAnsi = "宋体", EastAsia = "宋体" }),
    new Text(text) { Space = SpaceProcessingModeValues.Preserve });

void AppendIns(Paragraph para, string text) => para.Append(new InsertedRun(
    new Run(new Text(text) { Space = SpaceProcessingModeValues.Preserve }))
    { Id = NextId().ToString(), Author = Author, Date = date });

void AppendDel(Paragraph para, string text) => para.Append(new DeletedRun(
    new Run(new DeletedText(text) { Space = SpaceProcessingModeValues.Preserve }))
    { Id = NextId().ToString(), Author = Author, Date = date });

// 字符级 diff 进段落（replace 段落的行内对齐）
void DiffInto(Paragraph para, string a, string b)
{
    foreach (var op in DiffText(a, b))
    {
        switch (op.Tag)
        {
            case "equal": para.Append(PlainRun(a.Substring(op.A0, op.A1 - op.A0))); break;
            case "delete": AppendDel(para, a.Substring(op.A0, op.A1 - op.A0)); break;
            case "insert": AppendIns(para, b.Substring(op.B0, op.B1 - op.B0)); break;
            default: // replace
                AppendDel(para, a.Substring(op.A0, op.A1 - op.A0));
                AppendIns(para, b.Substring(op.B0, op.B1 - op.B0));
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
                if (orig[i].Length > 0) para.Append(PlainRun(orig[i]));
            }
            break;
        case "delete":
            for (int i = seg.A0; i < seg.A1; i++)
            {
                var para = body.AppendChild(new Paragraph());
                if (orig[i].Length > 0) AppendDel(para, orig[i]);
            }
            break;
        case "insert":
            for (int j = seg.B0; j < seg.B1; j++)
            {
                var para = body.AppendChild(new Paragraph());
                if (revised[j].Length > 0) AppendIns(para, revised[j]);
            }
            break;
        default: // replace
        {
            int n = Math.Min(seg.A1 - seg.A0, seg.B1 - seg.B0);
            for (int k = 0; k < n; k++)
            {
                var para = body.AppendChild(new Paragraph());
                DiffInto(para, orig[seg.A0 + k], revised[seg.B0 + k]);
            }
            for (int i = seg.A0 + n; i < seg.A1; i++)
            {
                var para = body.AppendChild(new Paragraph());
                if (orig[i].Length > 0) AppendDel(para, orig[i]);
            }
            for (int j = seg.B0 + n; j < seg.B1; j++)
            {
                var para = body.AppendChild(new Paragraph());
                if (revised[j].Length > 0) AppendIns(para, revised[j]);
            }
            break;
        }
    }
}

mainPart.Document.Save();
Console.WriteLine($"已生成（修订模式，{srcNote}）：{outPath}");
return 0;

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

readonly record struct Opcode(string Tag, int A0, int A1, int B0, int B1);
