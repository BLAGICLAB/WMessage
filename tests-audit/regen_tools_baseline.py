#!/usr/bin/env python3
"""
tools_baseline.json 重生成器（T1-QUERYTASKS 起常备）：

registry_tests::tools_json_matches_baseline 锁「tools_json() 前 28 项与 fixture 一致」。
核心 schema 显式变更（改名/扩参/描述修订）后运行本脚本重生成 fixture——
这是该锁设计的显式变更流程，重生成必须与 schema 改动同一提交，diff 里可 review。

流程：从 src-tauri/src/bot/registry.rs 抽取 SCHEMA_* 常量原文（不做 JSON parse +
re-serialize，保证字节保真），按 TOOLS_TABLE 前 28 项（核心 28）顺序拼装，
覆写 src-tauri/tests/fixtures/tools_baseline.json。

执行：python3 tests-audit/regen_tools_baseline.py
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "src-tauri/src/bot/registry.rs"
FIXTURE = ROOT / "src-tauri/tests/fixtures/tools_baseline.json"

# TOOLS_TABLE 前 28 项（核心 28；其后是编排三工具 + 2 个子 agent 专属，不进 baseline）
CORE_ORDER = [
    "QUERY_TASKS",
    "QUERY_SINGLE_TASK",
    "CREATE_TASK",
    "COMPLETE_TASK",
    "DELETE_TASK",
    "EDIT_TASK",
    "ADD_SUBTASK",
    "TOGGLE_SUBTASK",
    "REMOVE_SUBTASK",
    "READ_TEXT_FILE",
    "OCR_IMAGE",
    "GREP_FILES",
    "LIST_FILES",
    "LINK_FILE_TO_TASK",
    "EXTRACT_DOCUMENT",
    "CREATE_WORD",
    "CREATE_WORD_REVISIONS",
    "CREATE_EXCEL",
    "CREATE_PPT",
    "CREATE_PDF",
    "RUN_PYTHON",
    "WEB_SEARCH",
    "FETCH_URL",
    "GET_CURRENT_TIME",
    "REMEMBER_FACT",
    "RECALL_FACTS",
    "RECORD_LESSON",
    "USE_SKILL",
]


def main() -> None:
    src = REGISTRY.read_text()
    schemas = dict(re.findall(r'pub const SCHEMA_(\w+): &str = r##"(.*?)"##;', src, re.S))
    missing = [n for n in CORE_ORDER if n not in schemas]
    assert not missing, f"registry.rs 缺常量 SCHEMA_{missing}"

    body = ",\n  ".join(schemas[n] for n in CORE_ORDER)
    FIXTURE.write_text(f'const TOOLS: &str = r#"[\n  {body}\n]"#;\n')
    print(f"OK: {FIXTURE.relative_to(ROOT)} 重生成（{len(CORE_ORDER)} 项）")
    # 抽取的 schema 必须都能 parse（防 raw string 截断）
    import json

    for n in CORE_ORDER:
        json.loads(schemas[n])
    print("OK: 28 项 schema 均为合法 JSON")


if __name__ == "__main__":
    main()
