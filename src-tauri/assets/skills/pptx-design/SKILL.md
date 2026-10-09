---
name: pptx-design
description: PPT 设计规范 + python-pptx 自由绘制指南（配色系统/版式原型/中文排印/密度硬规范/完工自查）。用户要求设计感、品牌感，或需要图表/大数字/双栏/引用等高级版式时装载本技能；简单陈述型 PPT 直接用 create_ppt。
version: 1.0.0
risk_level: low
mode: auto
enabled: true
intents: ["PPT", "ppt", "幻灯片", "演示文稿", "汇报", "slide", "deck"]
---

# PPT 设计规范与自由绘制（python-pptx）

本技能教你用 `run_python` + python-pptx 从空白演示自由绘制有设计感的幻灯片。
设计质量来自本规范的确定性规则：配色有系统、版式有原型、密度有硬限、交付有自查。

## 0. 适用与分工

- 简单陈述型（标题 + 要点 + 表格）：直接用 `create_ppt` 工具，**不要装载本技能**。
- 装载本技能的场合：用户要设计感/品牌感/公司配色，或内容需要图表、大数字、
  双栏对比、引用页等高级版式。
- 执行方式：一段 `run_python` 代码生成整份 PPT；修复 = 改代码整体重跑（幂等，覆盖同一文件）。
- 本机没有 LibreOffice/PowerPoint 渲染通道：**不要尝试调用 soffice 截图**，
  交付前用第 7 节自查清单逐项过一遍代替看图。

## 1. 画布与交付

- 一律 16:9。新建的 Presentation 默认 4:3，必须显式设置：

```python
from pptx import Presentation
from pptx.util import Inches, Pt, Emu
prs = Presentation()
prs.slide_width = Inches(13.333)
prs.slide_height = Inches(7.5)
blank = prs.slide_layouts[6]   # 空白版式，一切元素自己画
```

- 输出文件写到 AI_Gen_Files 的绝对路径（路径见系统提示的「生成目录规则」），
  先 `os.makedirs(dirname, exist_ok=True)`；生成成功后 `print("已生成：" + 绝对路径)`。
- 任务卡执行流程内（🤖/⏰/📦）：再用 `link_file_to_task` 登记产物；普通对话不用。
- 一份 10 页左右的 PPT 正常 30 秒内完成；单次 run_python 上限 300 秒，不要分多次
  增量保存同一文件（每轮都整份重生成）。

## 2. 中文渲染必做（漏了中文必出方块或回退宋体）

`font.name` 只写西文字形槽，**中文字形必须补 eastAsia**。全代码共用这一段辅助函数：

```python
from pptx.oxml.ns import qn

def set_font(run, name="Microsoft YaHei", size=None, bold=False, color=None):
    f = run.font
    f.name = name                      # 西文/数字
    f.bold = bold
    if size: f.size = Pt(size)
    if color: f.color.rgb = RGBColor.from_string(color)
    rpr = run._r.get_or_add_rPr()
    ea = rpr.find(qn("a:ea"))
    if ea is None:
        ea = rpr.makeelement(qn("a:ea"), {})
        rpr.append(ea)
    ea.set("typeface", name)           # 中文字形
```

字体只用「Microsoft YaHei」（微软雅黑）：Windows 原生；macOS 无此字体时 PowerPoint
自动回退苹方，观感正常。**不要**按平台写双字体，也**不要**用宋体/楷体做正文。

## 3. 设计五原则

1. **明暗三明治**：封面、章节页、结尾页用深色整底；内容页浅底。全程深色或全程浅色
   也可以，但不许混搭无规律。
2. **一页一主张**：一页只讲一件事。这页的标题就是这句话；讲稿细节写进备注
   （`slide.notes_slide.notes_text_frame.text = "..."`），不放正文。
3. **60-30-10**：约 60-70% 面积是底色与中性色；主色用于大块面（整页底、半版色块、
   表头整行）；强调色 ≤10% 只做点缀（数字、关键词、图表高亮）。主色+强调色之外的
   颜色不许出现。
4. **视觉母题**：选一个贯穿元素（如圆角卡片、超大数字、细分割线中的一种），每页
   复用同一种，形成整体感。不要每页发明新花样。
5. **每页一个视觉锚点**：大数字、图表、色块卡片、表格、图片——至少占版面 1/4。
   纯文字页是事故。相邻两页不许用同一种版式原型。

明确禁止（AI 味重灾区）：标题下加横线或短色条；页面边缘装饰竖条/色带；彩虹配色；
整页大段文字；emoji 当图标。

## 4. 配色系统（8 组，每组 4 色）

`bg` 内容页底 / `band` 深色整底（封面·章节·表头） / `accent` 强调 / `text` 正文。

| 组名 | bg | band | accent | text | 适用 |
|---|---|---|---|---|---|
| 午夜商务 | F7F8FA | 0F1E33 | C9A227 | 1A1A1A | 董事会/金融/年报 |
| 靛蓝科技 | F6F8FC | 1E3A8A | F59E0B | 1F2937 | AI/云计算/产品发布 |
| 松林绿 | FAFAF6 | 1E4D2B | D97706 | 24312A | 环保/农业/可持续 |
| 暖沙学院 | FDF8F0 | 7C2D12 | B45309 | 292524 | 学术/人文/历史 |
| 石墨夜场 | 16181D | 2B2F3A | 4A90D9 | E8EAED | 发布会/夜间场（全程深色） |
| 青碧现代 | F6FAFA | 00565B | 0E7490 | 15343A | 医疗/健康/康养 |
| 绛紫轻奢 | FAF7FB | 3B1F4E | B08D57 | 2B2430 | 珠宝/高端咨询/奢侈 |
| 海蓝清爽 | F4F9FF | 075985 | D97706 | 1E293B | 旅游/教育/日常汇报 |

选组规则：用户给了品牌色 → 以品牌色为 band 重新配 accent（对比色或同族亮色）；
没给 → 按场合选上表。深色底（band）上的文字永远是白色或该组 bg 色；正文用深灰
（text 列），不用纯黑。python-pptx 不支持形状透明度，半透明效果用「band 色混入
bg 的浅色变体」近似（如 band=0F1E33 的浅变体取 E7EBF2）。

## 5. 字号与密度硬规范

| 元素 | 字号 | 说明 |
|---|---|---|
| 章节大标题 | 40-54pt 加粗 | 章节页专用 |
| 页面标题 | 28-36pt 加粗 | 每页一句结论式标题 |
| 正文要点 | 14-18pt | 行距 1.15-1.3 |
| 注释/来源 | 11-12pt | 弱色（text 加 50% 灰度的变体） |
| 大数字 | 60-96pt 加粗 | KPI/数据锚点，配 accent 或 band 色 |

密度硬限（防文字堆砌，超限就拆页，不许缩字号硬塞）：

- 每页要点 ≤5 条，每条 ≤40 个字；
- 整页正文字数 ≤100 字（不含标题/注释）；
- 讲稿、推演、证据 → 全部放备注，不放页面。

## 6. 版式原型与骨架代码

以下片段都基于第 1-2 节的 `prs/blank/set_font` 与选定的 T = dict(bg=..., band=...,
accent=..., text=...)。矩形辅助：

```python
from pptx.enum.shapes import MSO_SHAPE
from pptx.dml.color import RGBColor
from pptx.enum.text import PP_ALIGN, MSO_ANCHOR

def rect(s, x, y, w, h, color, rounded=False):
    sp = s.shapes.add_shape(
        MSO_SHAPE.ROUNDED_RECTANGLE if rounded else MSO_SHAPE.RECTANGLE, x, y, w, h)
    sp.fill.solid(); sp.fill.fore_color.rgb = RGBColor.from_string(color)
    sp.line.fill.background(); sp.shadow.inherit = False
    return sp

def textbox(s, x, y, w, h, text, size, color, bold=False, align=PP_ALIGN.LEFT,
            anchor=MSO_ANCHOR.TOP, spacing=1.2):
    tb = s.shapes.add_textbox(x, y, w, h)
    tf = tb.text_frame; tf.word_wrap = True; tf.vertical_anchor = anchor
    lines = text.split("\n")
    for i, line in enumerate(lines):
        p = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
        p.alignment = align; p.line_spacing = spacing
        r = p.add_run(); r.text = line
        set_font(r, size=size, bold=bold, color=color)
    return tb
```

**A 封面**（深底）：`rect(0,0,W,H,band)`；主标题 textbox 居左 1.1in、上 2.4in、
40-48pt 白色加粗；副标题 18-20pt 浅色；底部一行日期/署名 12pt。

**B 章节页**（band 整底）：大号「02」式序号（accent 色 60pt）+ 章节标题 40-54pt 白色，
居中或左对齐；下一行一句本章节要回答的问题（16pt 浅色）。

**C 要点页**（浅底）：左侧 1/3 放标题区（结论式标题 30pt + 一句补充 14pt 弱色），
右侧 2/3 放要点（每条前加 band 色小方块或粗点，14-18pt，行距 1.3，条间距用空段落）。
要点超过 5 条 → 拆成两页（第二页标题加「（续）」）。

**D 双栏对比**（浅底）：页标题一行；下方两个圆角卡片（`rounded=True`，底色用 band
的浅变体，卡片标题用 band 色 16pt 加粗），左右各半，中间留 0.4in；每张卡内要点
≤4 条。适合「方案 A vs B」「现状 vs 目标」。

**E 数据大字页**（浅底）：2-4 张并排卡片，每张 = 大数字（60-96pt accent/band 色）
+ 一行标签（14pt）+ 一行说明（12pt 弱色）。数字必须真实（来自任务数据），不许编造。

**F 图表页**（浅底）：页标题 + 图表占版面 2/3。

```python
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION
cd = CategoryChartData()
cd.categories = ["一季度", "二季度", "三季度"]
cd.add_series("营收(万元)", (120, 158, 143))
gf = s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED,
                        Inches(0.9), Inches(1.6), Inches(8.2), Inches(5.0), cd)
ch = gf.chart; ch.has_legend = False; ch.has_title = False
```

柱/条用 COLUMN_CLUSTERED/BAR_CLUSTERED，趋势用 LINE_MARKERS，占比用 PIE（≤5 块）。
图表字体随主题：`ch.font.size = Pt(12)`；系列色：`plot.series[0].format.fill...` 设
band 色，高亮项设 accent 色。表格页同理：`shapes.add_table` 后表头整行 band 底白字、
数据行白底深灰字、斑马纹用 bg 的相邻变体，字号 12-14pt。

**G 结尾页**（band 整底）：居中「谢谢 / Q&A」40pt + 一行行动号召或联系方式 14pt。

**图片**：`s.shapes.add_picture(绝对路径, x, y, width=Inches(5.5))`——只插用户提供的
或此前已生成在 AI_Gen_Files 的图片，不要下载网络图片；宽度显式给定避免占满。

**页码**：内容页右下角 `i / total`（11pt 弱色），封面/章节/结尾页不放。

## 7. 完工自查清单（代替看图，逐项过了再交付）

1. 画布是 16:9（13.333×7.5）？
2. 每个中文 run 都走了 `set_font`（eastAsia）？逐页检查，漏一处中文就是宋体。
3. 配色只用选定组的 4 色？band 上的文字全是白/bg 色？
4. 有没有标题横线、边缘色条、emoji 图标？（有就删）
5. 密度：每页 ≤5 条、每条 ≤40 字、整页 ≤100 字？超限拆页了吗？
6. 相邻页版式不重复？每页有视觉锚点？
7. 溢出估算：中文字符宽≈字号(pt)/72 英寸，`字数×字宽×行数` ≤ 文本框宽×高？
   最长一行放得下吗？
8. 文件真的落盘了（`os.path.getsize(out) > 0`），print 了完整绝对路径？

自查发现问题的修复方式：改代码、整份重跑、覆盖同一文件，不要手动补救单个文件。

## 8. 常见坑

- **无透明度 API**：`fill.fore_color` 没有 transparency；浅色变体代替（见第 4 节）。
- **默认 4:3**：忘改画布尺寸是最高频事故（第 1 节第一件事）。
- **只设 font.name**：中文不生效，必须 eastAsia（第 2 节）。
- **print 二进制/整页文本**：工具返回有长度上限，只 print 进度与最终路径。
- **别用 emoji/特殊字符当图标**：跨平台渲染不稳，用色块、形状、大数字表达。
- **别在代码里写死用户没给的数字**：数据来自任务上下文或用户输入，缺数据就少一页，
  不要编。
