#!/usr/bin/env python3
"""验收方案 ↔ SPEC ↔ 卡片 三方对拍门禁：

- 真相源 A：`SPEC.md`「功能规格」节（114 条功能条目，88 主 + 26 子）
- 真相源 B：`docs/acceptance/matrix.csv`（追踪矩阵，每行一条功能）
- 真相源 C：`docs/acceptance/cards-*.md`（逐条验收卡）
- 真相源 D：`src-tauri/src/**` + `src/**`（审计事件名的存在性）

查五件事：

1. **功能条目对齐**：矩阵的功能 ID 集合与 SPEC 解析出的集合**逐条相等**（不是只比数量——
   数量相等但错位是最难发现的漂移）。
2. **每条功能有卡**：矩阵每行的卡号在卡文件里能找到对应标题行。
3. **卡号唯一**：一个卡号不能被两条功能共用，也不能在卡文件里出现两次。
4. **卡 ↔ 功能双向对住**：卡里标注的 SPEC 编号与矩阵该行的功能 ID 一致（防张冠李戴）。
5. **判据里的事件名真实存在**：卡里出现的 `a.b` 形事件名必须在源码里存在。

**为什么要有这道闸**：验收方案最容易腐化的方式不是没人写，而是**方案与代码分家**——
功能改了、卡没改，执行者在机器上 grep 不到那一行，只能靠猜判断是「应用坏了」还是
「文档过期」。第 1 条挡住「SPEC 加了功能没人出卡」，第 5 条挡住「卡里写了从未存在的事件」。

执行：python3 -m pytest tests-audit/audit_acceptance_matrix.py -v
"""

from pathlib import Path
import csv
import re
import sys

import pytest

ROOT = Path(__file__).resolve().parent.parent
MATRIX = ROOT / "docs/acceptance/matrix.csv"
CARDS_DIR = ROOT / "docs/acceptance"
SPEC = ROOT / "SPEC.md"

sys.path.insert(0, str(Path(__file__).resolve().parent))
from audit_doc_claims import (  # noqa: E402  —— 事件名规则单源，不另立一套
    EVENT_RE,
    FILE_EXTS,
    NOT_EVENTS,
    in_source,
    source_blob,
)

# SPEC「功能规格」节的解析规则与矩阵生成脚本一致：加粗条目与子条目都算一条功能
SECTION_RE = re.compile(r"^### (\d+)\. (.+)")
BOLD_RE = re.compile(r"^- \*\*(.+?)\*\*")
PLAIN_BULLET_RE = re.compile(r"^- ")

CARD_HEADING_RE = re.compile(r"^###\s+(ACC-[A-Z]-\d{2})\b")
# 功能 ID 是「节号-节内流水号」，节内流水号到三位（5-103 / 6-106）——只截两位会把 5-103 读成 5-10
CARD_SPEC_RE = re.compile(r"\*\*SPEC\*\*[：:]\s*(\d+-\d{2,3})")

VALID_STATUS = {"未验", "通过", "失败", "阻塞", "豁免"}
VALID_LAYER = {"L0", "L1", "L2", "L3", "L1+L2", "L0+L1", "L2+L3", "L1+L2+L3"}


def spec_feature_ids() -> list[str]:
    """SPEC「功能规格」节里的功能条目 ID（按出现顺序）。"""
    lines = SPEC.read_text(encoding="utf-8").split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith("## 功能规格"))
    end = next(i for i, l in enumerate(lines) if l.startswith("## 技术栈"))
    sec_no = 0
    item_no = 0
    ids = []
    for line in lines[start:end]:
        m = SECTION_RE.match(line)
        if m:
            sec_no = int(m.group(1))
            continue
        if BOLD_RE.match(line) or PLAIN_BULLET_RE.match(line):
            item_no += 1
            ids.append(f"{sec_no}-{item_no:02d}")
    return ids


def matrix_rows() -> list[dict]:
    with MATRIX.open(encoding="utf-8") as f:
        return list(csv.DictReader(f))


def card_files() -> list[Path]:
    return sorted(CARDS_DIR.glob("cards-*.md"))


def card_headings() -> dict[str, tuple[Path, int, str]]:
    """卡号 → (文件, 行号, 该卡全文)。"""
    out = {}
    for path in card_files():
        text = path.read_text(encoding="utf-8")
        lines = text.split("\n")
        starts = [(i, m.group(1)) for i, l in enumerate(lines) if (m := CARD_HEADING_RE.match(l))]
        for idx, (i, cid) in enumerate(starts):
            end = starts[idx + 1][0] if idx + 1 < len(starts) else len(lines)
            out.setdefault(cid, (path, i + 1, "\n".join(lines[i:end])))
    return out


def test_matrix_covers_every_spec_feature():
    spec_ids = spec_feature_ids()
    row_ids = [r["功能ID"] for r in matrix_rows()]
    assert spec_ids, "SPEC 的功能规格节没解析出任何条目——解析规则变了？"
    missing = [i for i in spec_ids if i not in row_ids]
    extra = [i for i in row_ids if i not in spec_ids]
    assert not missing, "SPEC 里有功能条目没进矩阵：\n" + "\n".join(missing)
    assert not extra, "矩阵里有 SPEC 里不存在的功能 ID：\n" + "\n".join(extra)
    assert len(row_ids) == len(set(row_ids)), "矩阵里有重复的功能 ID"


def test_every_feature_has_a_card():
    cards = card_headings()
    missing = []
    for row in matrix_rows():
        cid = (row.get("新卡号") or "").strip()
        if not cid:
            missing.append(f"{row['功能ID']}（卡号为空）")
        elif cid not in cards:
            missing.append(f"{row['功能ID']} → {cid}（卡文件里找不到该卡）")
    if missing:
        pytest.fail(
            "有功能条目没有验收卡：\n  " + "\n  ".join(missing) + "\n（每个功能条目都要一张卡）"
        )


def test_card_ids_are_unique():
    seen = {}
    dupes = []
    for path in card_files():
        for i, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            m = CARD_HEADING_RE.match(line)
            if not m:
                continue
            cid = m.group(1)
            if cid in seen:
                dupes.append(f"{cid}（{seen[cid]} 与 {path.name}:{i}）")
            else:
                seen[cid] = f"{path.name}:{i}"
    if dupes:
        pytest.fail("卡号重复：\n  " + "\n  ".join(dupes))


def test_card_spec_id_matches_matrix():
    cards = card_headings()
    bad = []
    for row in matrix_rows():
        cid = (row.get("新卡号") or "").strip()
        if cid not in cards:
            continue
        _, _, body = cards[cid]
        m = CARD_SPEC_RE.search(body)
        if not m:
            bad.append(f"{cid}（卡里没标 **SPEC**：编号）")
        elif m.group(1) != row["功能ID"]:
            bad.append(f"{cid}（卡标 {m.group(1)}，矩阵行是 {row['功能ID']}）")
    if bad:
        pytest.fail(
            "卡与矩阵指向的功能不一致：\n  "
            + "\n  ".join(bad)
            + "\n（一个卡只能对一个功能；矩阵与卡必须同步改）"
        )


def test_matrix_columns_are_well_formed():
    bad = []
    for row in matrix_rows():
        fid = row.get("功能ID", "?")
        status = (row.get("状态") or "").strip()
        if status not in VALID_STATUS:
            bad.append(f"{fid} 状态值「{status}」不在 {sorted(VALID_STATUS)}")
        layer = (row.get("验收层") or "").strip()
        if layer not in VALID_LAYER:
            bad.append(f"{fid} 验收层「{layer}」不在 {sorted(VALID_LAYER)}")
        if not (row.get("缺口说明") or "").strip():
            bad.append(f"{fid} 缺缺口说明（每行都要写清已知缺口或「—」）")
    if bad:
        pytest.fail("矩阵列值不合规：\n  " + "\n  ".join(bad))


def test_matrix_referenced_paths_exist():
    """矩阵「自动化覆盖」列里写的每个文件路径都必须真实存在。

    为什么查这条：路径是执行者的入口——写错一个目录层级，执行者按矩阵去找会扑空，
    然后误以为「这个功能没有自动化覆盖」，把该补的测试漏掉。路径漂移不报错，
    只会静静地把验收口径改小。
    """
    path_re = re.compile(r"(?:src-tauri|src|scripts|docs|tests-audit)/[A-Za-z0-9_./-]+\.[A-Za-z0-9_]+")
    missing = []
    for row in matrix_rows():
        col = (row.get("自动化覆盖") or "")
        for token in path_re.findall(col):
            token = token.rstrip(".,;")
            if not (ROOT / token).exists():
                missing.append(f"{row['功能ID']}  {token}")
    if missing:
        pytest.fail(
            "矩阵「自动化覆盖」列里的路径在仓库里不存在：\n  "
            + "\n  ".join(missing)
            + "\n（路径写错 = 执行者按矩阵去找会扑空 = 该补的测试被漏掉）"
        )


def test_cards_use_the_real_data_dir():
    """卡片里的数据目录必须是真身，别让执行者对着空路径 grep。

    macOS 上数据目录是 `~/Library/Application Support/com.renshi.wmessage/`
    （Tauri `app_data_dir()` + 标识 `com.renshi.wmessage`），便携模式另走
    `paths.rs::probe_dir` 的 exe 目录锚定 / `app_data_dir` 回退两条路。
    `~/.wmessage/` 这种写法看着像但根本不存在——照着 grep 只会得到空输出，
    然后被误读成「事件没落账」。
    """
    wrong = []
    for path in card_files():
        for lineno, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            if "~/.wmessage" in line:
                wrong.append(f"{path.name}:{lineno}")
    if wrong:
        pytest.fail(
            "卡片里用了不存在的数据目录写法 `~/.wmessage/`：\n  "
            + "\n  ".join(wrong)
            + "\n（真身：~/Library/Application Support/com.renshi.wmessage/ —— "
            "便携模式另走 exe 目录锚定，见 paths.rs::probe_dir）"
        )


def test_card_referenced_paths_exist():
    """卡片里引用的仓库内路径必须真实存在（失败处置那一行靠它定位实现）。"""
    path_re = re.compile(r"(?:src-tauri|src|scripts|docs|tests-audit)/[A-Za-z0-9_./-]+\.[A-Za-z0-9_]+")
    missing = []
    for path in card_files():
        for lineno, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            for token in path_re.findall(line):
                token = token.rstrip(".,;")
                if not (ROOT / token).exists():
                    missing.append(f"{path.name}:{lineno}  {token}")
    if missing:
        pytest.fail(
            "卡片引用的仓库路径不存在：\n  "
            + "\n  ".join(missing)
            + "\n（失败处置那一行靠路径定位实现；路径失效 = 出问题时无从查起）"
        )


def test_event_names_in_cards_exist_in_source():
    blob = source_blob()
    cands = []
    for path in card_files():
        for lineno, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            for span in re.findall(r"`([^`\n]+)`", line):
                if not EVENT_RE.match(span):
                    continue
                if span.endswith(FILE_EXTS) or "www." in span:
                    continue
                if span in NOT_EVENTS:
                    continue
                cands.append((path.name, lineno, span))
    assert cands, "没解析到任何审计事件名——卡片的引用写法变了吗？"
    missing = [(f, n, t) for f, n, t in cands if not in_source(t, blob)]
    if missing:
        detail = "\n".join(f"  {f}:{n}  {t}" for f, n, t in missing)
        pytest.fail(
            "卡片引用的审计事件名在源码里不存在：\n"
            + detail
            + "\n（要么代码改了名——改卡；要么卡里写了从未存在的事件——核对来源）"
        )
