"""Real Tool Executor 测试 — doc_make_word 真生成文件。"""
from __future__ import annotations

import os

import pytest

from agent_core.real_executor import RealToolExecutor


# ───────────────────────── doc_make_word 真生成 ─────────────────────────


def test_doc_make_word_creates_file(tmp_path):
    """doc_make_word 真生成 .docx 文件。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_word", {
        "title": "Test Document",
        "paragraphs": ["第一段", "第二段", ""],
    })
    assert "已生成：" in result

    # 找出真实文件路径
    files = list(tmp_path.glob("*.docx"))
    assert len(files) == 1
    path = files[0]
    assert path.exists()
    assert os.path.getsize(path) > 0  # 非空


def test_doc_make_word_with_tables(tmp_path):
    """doc_make_word 含表格。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_word", {
        "title": "表格测试",
        "paragraphs": ["前面段"],
        "tables": [{
            "title": "数据表",
            "rows": [["列1", "列2"], ["值1", "值2"], ["值3", "值4"]],
        }],
    })
    assert "已生成：" in result
    files = list(tmp_path.glob("*.docx"))
    assert len(files) == 1
    assert files[0].stat().st_size > 1000  # 含表格 → 较大


def test_doc_make_word_with_filename(tmp_path):
    """指定文件名。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_word", {
        "title": "x",
        "paragraphs": ["y"],
        "filename": "custom-name.docx",
    })
    assert "custom-name.docx" in result
    assert (tmp_path / "custom-name.docx").exists()


def test_doc_make_word_empty_args(tmp_path):
    """空 args 也行（生成最小 docx）。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_word", {})
    assert "已生成：" in result
    files = list(tmp_path.glob("*.docx"))
    assert len(files) == 1


def test_doc_make_word_creates_output_dir(tmp_path):
    """output_dir 不存在 → 自动创建。"""
    new_dir = tmp_path / "nested" / "deeper" / "out"
    assert not new_dir.exists()
    executor = RealToolExecutor(output_dir=str(new_dir))
    executor.execute("doc_make_word", {"title": "x", "paragraphs": ["y"]})
    assert new_dir.is_dir()
    assert any(new_dir.glob("*.docx"))


# ───────────────────────── doc_make_excel/ppt/pdf stub ─────────────────────────


def test_doc_make_excel_creates_file(tmp_path):
    """doc_make_excel stub — Phase 3 生成 PK 占位文件。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_excel", {"sheets": [{"name": "Sheet1", "rows": [["a", "b"]]}]})
    assert "已生成：" in result
    files = list(tmp_path.glob("*.xlsx"))
    assert len(files) == 1


def test_doc_make_ppt_creates_file(tmp_path):
    """doc_make_ppt stub。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_ppt", {
        "title": "演示",
        "slides": [{"type": "cover", "title": "X"}],
    })
    assert "已生成：" in result
    files = list(tmp_path.glob("*.pptx"))
    assert len(files) == 1


def test_doc_make_pdf_creates_file(tmp_path):
    """doc_make_pdf stub。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("doc_make_pdf", {
        "title": "x",
        "paragraphs": ["y"],
    })
    assert "已生成：" in result
    files = list(tmp_path.glob("*.pdf"))
    assert len(files) == 1


# ───────────────────────── fallback 行为 ─────────────────────────


def test_unknown_tool_falls_back_to_mock():
    """未知工具 → fallback 到 LocalVerifyExecutor mock。"""
    executor = RealToolExecutor(output_dir="/tmp")
    result = executor.execute("unknown_tool", {"arg": "value"})
    # LocalVerifyExecutor.execute 返回 mock execute 字符串
    assert "unknown_tool" in result


def test_list_tasks_falls_back():
    """list_tasks → mock 返回含 UUID。"""
    executor = RealToolExecutor(output_dir="/tmp")
    result = executor.execute("list_tasks", {})
    # LocalVerifyExecutor.execute for "list_tasks" returns mock data
    assert "00000000-0000-4000-8000-000000000001" in result


# ───────────────────────── Verify 4 种 kind 真实执行 ─────────────────────────


def test_verify_command_real_execution(tmp_path):
    """RealToolExecutor.run_command 真跑命令（继承自 LocalVerifyExecutor）。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    from agent_core.verify import verify
    from agent_core.types import TaskNode, VerifyHint

    task = TaskNode(
        id="t1", title="x",
        verify_hint=VerifyHint(kind="command", command="echo done", expect="exit_0"),
    )
    r = verify(task, executor)
    assert r.passed is True


def test_verify_file_hash_real_execution(tmp_path):
    """RealToolExecutor.file_hash 真算 SHA256。"""
    f = tmp_path / "test.txt"
    f.write_text("hello")
    import hashlib

    executor = RealToolExecutor(output_dir=str(tmp_path))
    assert executor.file_hash(str(f)) == hashlib.sha256(b"hello").hexdigest()


def test_verify_assert_real_execution(tmp_path):
    """RealToolExecutor.eval_assert 真求值。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    assert executor.eval_assert("1+1") == 2


def test_verify_llm_judge_still_stub(tmp_path):
    """RealToolExecutor.llm_judge 仍是 stub（不接 LLM）。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    passed, score, _ = executor.llm_judge("rubric", "output", 0.7)
    assert passed is False
    assert score == 0.0


# ───────────────────────── 与 LocalVerifyExecutor 接口兼容 ─────────────────────────


def test_is_verify_executor():
    """RealToolExecutor 兼容 VerifyExecutor 接口（agent_loop 需要）。"""
    from agent_core.verify import VerifyExecutor
    executor = RealToolExecutor(output_dir="/tmp")
    assert isinstance(executor, VerifyExecutor)


def test_runs_calls_recorded(tmp_path):
    """execute() 调用都记入 calls 列表。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    executor.execute("doc_make_word", {"title": "x", "paragraphs": ["y"]})
    executor.execute("unknown_tool", {})  # fallback mock
    executor.execute("list_tasks", {})     # fallback mock via LocalVerifyExecutor
    assert len(executor.calls) == 3
    # 每条都记到 name + args
    for call in executor.calls:
        assert call["method"] == "execute"
        assert "name" in call
        assert "args" in call


def test_execute_with_none_args(tmp_path):
    """execute(name, None) → 不崩。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    result = executor.execute("unknown_tool", None)
    assert "unknown_tool" in result


def test_execute_with_string_args(tmp_path):
    """execute(name, "string") → 兜底 dict。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    # 调 doc_make_word 但 args 是 string（异常情况）— 应走 fallback mock
    result = executor.execute("unknown_tool", "not a dict")
    assert "unknown_tool" in result


# ───────────────────────── 集成：doc_make_word + 文件验证 ─────────────────────────


def test_docx_file_is_valid_zip(tmp_path):
    """生成的 .docx 实际是合法 zip（python-docx 输出）。"""
    executor = RealToolExecutor(output_dir=str(tmp_path))
    executor.execute("doc_make_word", {"title": "t", "paragraphs": ["p"]})

    files = list(tmp_path.glob("*.docx"))
    assert len(files) == 1
    content = files[0].read_bytes()
    # .docx 是 zip，PK\x03\x04 是 zip 魔数
    assert content[:4] == b"PK\x03\x04"


def test_docx_contains_text(tmp_path):
    """生成的 .docx 包含 title + paragraphs 文本（解 zip 验证）。"""
    import zipfile
    import xml.etree.ElementTree as ET

    executor = RealToolExecutor(output_dir=str(tmp_path))
    executor.execute("doc_make_word", {
        "title": "MyTitle",
        "paragraphs": ["ParaOne", "ParaTwo"],
    })

    files = list(tmp_path.glob("*.docx"))
    with zipfile.ZipFile(files[0]) as z:
        with z.open("word/document.xml") as f:
            content = f.read().decode("utf-8")
    assert "MyTitle" in content
    assert "ParaOne" in content
    assert "ParaTwo" in content
