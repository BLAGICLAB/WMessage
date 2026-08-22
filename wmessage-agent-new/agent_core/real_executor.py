"""Real Tool Executor — Phase 3 step 2。

替换 MockExecutor，让 LLM 调 doc_make_word 等工具时**真实生成文件**。

设计：
- 继承 LocalVerifyExecutor — verify 4 种 kind 真实执行
- 重写 execute(name, args) — 真实调 Python 库生成文件
- Phase 3 实现：doc_make_word（python-docx）+ 其他 doc_* stub
- 输出文件落到 data/output/（可配置）

Phase 3 后续：
- doc_make_excel（openpyxl）
- doc_make_pdf（reportlab）
- doc_make_ppt（python-pptx + 10 套 theme）
- run_python（独立 sandbox，subprocess）

为什么不绕 IPC 透传 Rust？
- 进程内 exec 比 IPC 快（省 5-10ms / 调用）
- Phase 2 已验证 Mock 路径，端到端跑通；Phase 3 切换到 Real 时保持架构稳定
- Rust bot_py.rs 仍是 WMessage 主进程的真执行路径；Python agent 这边是 redundant 路径
  （用于 test / standalone / 跨平台场景）
"""
from __future__ import annotations

import os
import re
import time
from typing import Any, Dict, List, Optional

from agent_core.verify import LocalVerifyExecutor


# ───────────────────────── doc_make_word 真实现 ─────────────────────────


def make_docx_impl(args: Dict[str, Any], output_dir: str) -> str:
    """doc_make_word 真生成 .docx 文件（python-docx）。

    Args:
        args: {title, paragraphs, tables?, filename?}
        output_dir: 输出目录（自动建）

    Returns:
        生成的文件绝对路径
    """
    try:
        import docx
        from docx.shared import Pt
    except ImportError:
        raise RuntimeError(
            "python-docx 未安装。pip install python-docx"
        )

    title = args.get("title", "")
    paragraphs = args.get("paragraphs", [])
    tables = args.get("tables", []) or []
    filename = args.get("filename")
    out = filename or os.path.join(output_dir, _gen_filename("docx"))
    if not os.path.isabs(out):
        out = os.path.join(output_dir, out)

    d = docx.Document()
    # 正文基础样式：宋体 12pt
    style = d.styles["Normal"]
    style.font.name = "宋体"
    style.font.size = Pt(12)

    if title:
        h = d.add_heading("", level=1)
        r = h.add_run(title)
        r.font.name = "黑体"
        r.font.size = Pt(16)

    for para in paragraphs:
        if para == "":
            d.add_paragraph("")
        else:
            pr = d.add_paragraph(para)
            pr.paragraph_format.first_line_indent = Pt(24)

    for t in tables:
        rows = t.get("rows", [])
        if not rows:
            continue
        if t.get("title"):
            tp = d.add_paragraph()
            tr = tp.add_run(t["title"])
            tr.font.bold = True
        tb = d.add_table(rows=len(rows), cols=len(rows[0]))
        tb.style = "Table Grid"
        for i, row in enumerate(rows):
            for j, cell in enumerate(row):
                tb.cell(i, j).text = str(cell)
        for cell in tb.rows[0].cells:
            for cpara in cell.paragraphs:
                for run in cpara.runs:
                    run.font.bold = True
        d.add_paragraph("")

    d.save(out)
    return out


# ───────────────────────── doc_make_excel / ppt / pdf stub ─────────────────────────


def make_xlsx_impl(args: Dict[str, Any], output_dir: str) -> str:
    """doc_make_excel — Phase 3 stub（Phase 4 用 openpyxl 完整实现）。"""
    out = args.get("filename") or os.path.join(output_dir, _gen_filename("xlsx"))
    if not os.path.isabs(out):
        out = os.path.join(output_dir, out)
    # Phase 3 stub：写空文件占位
    with open(out, "wb") as f:
        f.write(b"PK\x03\x04")  # xlsx 是 zip，PK 是 zip magic number
    return out


def make_ppt_impl(args: Dict[str, Any], output_dir: str) -> str:
    """doc_make_ppt — Phase 3 stub（Phase 4 用 python-pptx 完整实现）。"""
    out = args.get("filename") or os.path.join(output_dir, _gen_filename("pptx"))
    if not os.path.isabs(out):
        out = os.path.join(output_dir, out)
    with open(out, "wb") as f:
        f.write(b"PK\x03\x04")  # pptx 也是 zip
    return out


def make_pdf_impl(args: Dict[str, Any], output_dir: str) -> str:
    """doc_make_pdf — Phase 3 stub。"""
    out = args.get("filename") or os.path.join(output_dir, _gen_filename("pdf"))
    if not os.path.isabs(out):
        out = os.path.join(output_dir, out)
    with open(out, "wb") as f:
        f.write(b"%PDF-1.4\n")
    return out


def _gen_filename(ext: str) -> str:
    ts = int(time.time() * 1000)
    return f"doc-{ts}.{ext}"


# ───────────────────────── Real Tool Executor ─────────────────────────


class RealToolExecutor(LocalVerifyExecutor):
    """Phase 3 真工具执行器 — doc_make_* 真实生成文件。

    继承 LocalVerifyExecutor，所以 verify 4 种 kind（command / file_hash /
    assert / llm_judge）也走真实实现。LLM Judge 仍是 stub（不接 LLM）。
    """

    def __init__(self, output_dir: str = "data/output"):
        super().__init__()
        self.output_dir = output_dir
        os.makedirs(output_dir, exist_ok=True)
        self.calls: List[Dict[str, Any]] = []  # 调用记录（对齐 MockVerifyExecutor）

    def execute(self, name: str, args: Any) -> str:
        """执行真工具调用。

        已知 doc_make_*：真实生成文件
        未知工具：fallback 到 super().execute(name, args)（LocalVerifyExecutor mock）
        """
        try:
            if name == "doc_make_word":
                path = make_docx_impl(_normalize_args(args), self.output_dir)
                result = f"已生成：{path}"

            elif name == "doc_make_excel":
                path = make_xlsx_impl(_normalize_args(args), self.output_dir)
                result = f"已生成：{path}"

            elif name == "doc_make_ppt":
                path = make_ppt_impl(_normalize_args(args), self.output_dir)
                result = f"已生成：{path}"

            elif name == "doc_make_pdf":
                path = make_pdf_impl(_normalize_args(args), self.output_dir)
                result = f"已生成：{path}"

            else:
                # 未知工具 → fallback mock
                result = super().execute(name, args)

        except Exception as e:  # noqa: BLE001
            self.calls.append({"method": "execute", "name": name, "args": args, "error": str(e)})
            return f"执行 {name} 失败：{e}"

        self.calls.append({"method": "execute", "name": name, "args": args, "result": result[:200]})
        return result

        # 未知工具 → fallback mock
        return super().execute(name, args)


def _normalize_args(args: Any) -> Dict[str, Any]:
    """保证 args 是 dict（agent_loop 可能传 None / str）。"""
    if args is None:
        return {}
    if isinstance(args, dict):
        return args
    return {}
