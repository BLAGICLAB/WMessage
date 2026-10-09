#!/usr/bin/env python3
"""
audit_evolution_layering.py — Evolution 域分层依赖方向守卫（批次 A §8 / 批复 §九）。

规则：
  1. strategy.rs（策略层实现）：禁止 std::fs、chrono 时钟、rand、Mutex、tauri、
     evolution::panel / evolution::policy（上下文层）依赖——策略必须纯。
  2. 上下文层文件（evolution/policy.rs、evolution/mod.rs）：禁止 `impl
     EvolutionPolicy`——上下文只做读配置/构造 ctx/拿锁/编排，不做业务判定。
  3. 数据层文件（evolution/derive.rs、proposal.rs、change/、candidate/ 除
     conflict.rs）：禁止 use evolution::strategy（数据层不依赖策略层；
     conflict.rs 是登记在案的委托壳，豁免）。
  4. evolution/ 全域禁止 thread_rng 与 rand::random/rng/seq/distributions、
     Os/Small/Std/ThreadRng 家族（随机源必须经 EvalContext 注入）。

误报处理（批复 §9.1）：剥离注释（// 与 /* */）与字符串字面量后匹配；
跳过 #[cfg(test)] 模块体；排除本脚本自身。

自测：`python3 audit_evolution_layering.py --selftest`——正例必须 pass、
六类违例 fixture 必须 fail、误报 fixture（注释/字符串/测试模块内关键词）
必须 pass。

退出码：0 = 全过；1 = 有违例/自测失败。
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
EVO = REPO_ROOT / "src-tauri" / "src" / "evolution"

STRATEGY_FILE = EVO / "strategy.rs"
CONTEXT_FILES = [EVO / "policy.rs", EVO / "mod.rs"]
DATA_GLOB = [
    *sorted((EVO / "change").glob("*.rs")),
    EVO / "derive.rs",
    EVO / "proposal.rs",
]
DATA_CONFLICT = EVO / "candidate" / "conflict.rs"  # 登记在案的委托壳
DATA_CANDIDATE = sorted((EVO / "candidate").glob("*.rs"))

# 委托壳文件（迁移映射登记：change/derive.rs；conflict.rs 已随死代码删除）。
# 规则 3 对它做**行级豁免**：只放行受认可的委托调用形式。2026-10-09 策略层
# 降级为自由函数后，合法形态 = gate_decision() + GateDecision 类型；
# 旧 trait 形态（DefaultEvolutionPolicy / layer_priority / impact_ord）已随
# 批 2.2 删除，从白名单移除。其他任何 strategy 引用照抓。
DELEGATE_FILES = {EVO / "change" / "derive.rs"}
DELEGATE_ALLOW = re.compile(
    r"crate::evolution::strategy::gate_decision\("
    r"|crate::evolution::strategy::GateDecision"
)

# 各规则：文件集合 → 禁止模式（正则，剥注释/字符串后匹配）
RULES = [
    (
        "策略层纯度",
        [STRATEGY_FILE],
        [
            # \b 左锚定：只认完整路径段，不吃 pre_evolution::panel / xtauri::
            # 这类后缀同形标识符（crate:: 前缀等真实路径不受影响）
            (r"\bstd::fs\b", "std::fs（文件 IO）"),
            (r"\bfs::", "fs::（文件 IO）"),
            (r"\bchrono::Utc::now", "时钟直读"),
            (r"\brand\b", "rand（随机源）"),
            (r"\bMutex\b", "Mutex（持锁）"),
            (r"\btauri::", "tauri（运行时依赖）"),
            (r"\bevolution::panel", "panel（消费端）"),
            (r"\bevolution::policy", "policy（上下文层）"),
        ],
    ),
    (
        "上下文层不做业务判定",
        CONTEXT_FILES,
        [
            # 放宽到泛型/带注解形态：`impl<T> EvolutionPolicy for X` 同样是
            # 业务判定进上下文层（\b 左锚定，不吃 xxxEvolutionPolicy 同形名）
            (r"impl\b[^;{]*?\bEvolutionPolicy\b", "impl EvolutionPolicy（业务判定进上下文层）"),
        ],
    ),
    (
        "数据层不依赖策略层",
        [p for p in DATA_GLOB + DATA_CANDIDATE if p != DATA_CONFLICT],
        [
            (r"evolution::strategy", "use evolution::strategy（数据层→策略层）"),
            # group use 形态：`use crate::evolution::{strategy, store};`
            # 不含连续的 `evolution::strategy`，单独成模式防漏报
            (r"evolution::\{[^}]*\bstrategy\b", "use evolution::{…strategy…}（数据层→策略层）"),
        ],
    ),
    (
        "全域禁随机源",
        sorted(EVO.rglob("*.rs")),
        [
            (r"thread_rng", "thread_rng（随机源必须经 EvalContext 注入）"),
            (r"\brand::(?:random|rng|seq|distributions)\b", "rand 随机 API（随机源必须经 EvalContext 注入）"),
            (r"\b(?:Os|Small|Std|Thread)Rng\b", "rand Rng 家族（随机源必须经 EvalContext 注入）"),
        ],
    ),
]

# ---- 文本预处理：剥注释与字符串，标记 cfg(test) 段 ----

# 原始字符串：r"…"/r#"…"#/br##"…"## 任意 # 定界符（b?r 前缀）
RAW_STR_RE = re.compile(r'(?:b?r)(#*)"')

def strip_comments_and_strings(text: str) -> str:
    """把注释与字符串字面量内容替换为空格，保留换行与长度（行号稳定）。
    原始字符串（含 r#"…"# 定界符）整体置空——内部引号不能当普通串边界，
    否则串内容外泄成幽灵源码（误报/漏报双向失真）。"""
    out = []
    i, n = 0, len(text)
    in_line_comment = in_block_comment = in_string = in_char = False
    while i < n:
        c = text[i]
        two = text[i:i + 2]
        if in_line_comment:
            out.append("\n" if c == "\n" else " ")
            if c == "\n":
                in_line_comment = False
            i += 1
        elif in_block_comment:
            out.append("\n" if c == "\n" else " ")
            if two == "*/":
                out.append("  ")
                i += 2
                in_block_comment = False
            else:
                i += 1
        elif in_string:
            out.append("\n" if c == "\n" else " ")
            if c == "\\":
                out.append(" ")
                i += 2
            else:
                if c == '"':
                    in_string = False
                i += 1
        elif in_char:
            out.append(" ")
            if c == "\\":
                out.append(" ")
                i += 2
            else:
                if c == "'":
                    in_char = False
                i += 1
        else:
            # 原始字符串（r#*/r##*/br#*）：左邻非标识符字符才尝试，
            # 避免 our_string 这类词中 r 误启；未闭合的吞到文件尾
            m = RAW_STR_RE.match(text, i)
            if m and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
                em = re.compile(r'"' + re.escape(m.group(1))).search(text, m.end())
                j = em.end() if em else n
                out.append("".join("\n" if ch == "\n" else " " for ch in text[i:j]))
                i = j
            elif two == "//":
                in_line_comment = True
                out.append("  ")
                i += 2
            elif two == "/*":
                in_block_comment = True
                out.append("  ")
                i += 2
            elif c == '"':
                in_string = True
                out.append(" ")
                i += 1
            elif c == "'":
                nxt = text[i + 1] if i + 1 < n else ""
                nxt2 = text[i + 2] if i + 2 < n else ""
                if nxt == "\\" or nxt2 == "'":
                    # char 字面量 'x' / '\n' 等——进入 char 模式
                    in_char = True
                    out.append(" ")
                    i += 1
                else:
                    # 生命周期 'a / 'static / '_——普通源码放行；若按 char 模式
                    # 处理会吞掉到下一个单引号前的全部源码（含字符串与换行），
                    # 既漏报违例又破坏行号稳定性
                    out.append(" ")
                    i += 1
            else:
                out.append(c)
                i += 1
    return "".join(out)


def strip_cfg_test_modules(text: str) -> str:
    """删掉 #[cfg(test)] 开头的 mod 块（花括号配对计数；测试内关键词不算违例）。"""
    # [^{}]*?（而非 \s*）：兼容单行/多行形态与夹层属性
    # （`#[cfg(test)] #[allow(..)] pub mod tests {`），花括号为界不会跨语句误配。
    # 谓词放宽覆盖组合形态：#[cfg(all(test, feature = "x"))] / #[cfg(any(test, …))]；
    # 更深嵌套谓词不支持（方向为漏剥=可能误报，出现时再扩）
    pattern = re.compile(
        r"#\[cfg\s*\(\s*(?:"
        r"(?:all|any)\s*\(\s*[^()]*\btest\b[^()]*\)"
        r"|[^()]*\btest\b[^()]*"
        r")\s*\)\s*\][^{}]*?mod\s+\w+\s*\{"
    )
    while True:
        m = pattern.search(text)
        if not m:
            return text
        depth = 1
        i = m.end()
        while i < len(text) and depth > 0:
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
            i += 1
        # 剥离体逐字符替换但保留换行——吞换行会让违例行号整体偏移（诊断性）
        text = (
            text[:m.start()]
            + "".join("\n" if ch == "\n" else " " for ch in text[m.start():i])
            + text[i:]
        )


def check_file(path: Path) -> list[str]:
    raw = path.read_text(encoding="utf-8", errors="replace")
    cleaned = strip_cfg_test_modules(strip_comments_and_strings(raw))
    is_delegate_shell = path in DELEGATE_FILES
    hits = []
    for rule_name, files, patterns in RULES:
        if path not in files:
            continue
        for pat, label in patterns:
            for m in re.finditer(pat, cleaned):
                if is_delegate_shell and rule_name.startswith("数据层"):
                    line_start = cleaned.rfind("\n", 0, m.start()) + 1
                    line_end = cleaned.find("\n", m.end())
                    line = cleaned[line_start:line_end if line_end != -1 else len(cleaned)]
                    if DELEGATE_ALLOW.search(line):
                        continue  # 登记的委托调用形式，行级豁免
                line_no = cleaned.count("\n", 0, m.start()) + 1
                hits.append(f"{path.relative_to(REPO_ROOT)}:{line_no}: {label}（{rule_name}）")
    return hits


def run_checks() -> list[str]:
    violations = []
    seen = set()
    for _, files, _ in RULES:
        for f in files:
            if f in seen or not f.exists():
                continue
            seen.add(f)
            violations.extend(check_file(f))
    return violations


# ---- 自测 fixtures（正例必须 pass / 违例必须 fail / 误报必须 pass）----

SELFTEST_PASS = """
use crate::evolution::proposal::ImpactLevel;
pub struct DefaultEvolutionPolicy;
// 注释里的 thread_rng 和 std::fs 不算违例
const HINT: &str = "字符串里的 rand / Mutex / tauri:: 也不算";
const RAW: &str = r#"原始串里 "thread_rng" 与 rand::random 不算"#;
const RAW2: &str = br##"多井号 "rand::seq" 与 OsRng 同理"##;
fn with_lifetime<'a>(x: &'a str) -> &'static str { "static" }
#[cfg(test)] mod single_line { use rand::thread_rng; }
#[cfg(all(test, feature = "evo"))] mod combo { use std::fs; }
#[cfg(any(test, feature = "x"))]
mod combo2 {
    use rand::seq::index;
}
#[cfg(test)]
mod tests {
    use std::fs; // 测试模块内的 std::fs 豁免
    #[test]
    fn t() { let _ = thread_rng(); }
}
"""

SELFTEST_FAIL_SAMPLES = [
    "let x = std::fs::read_to_string(\"p\");",
    "let now = chrono::Utc::now();",
    "use rand::thread_rng;",
    "static L: Mutex<()> = Mutex::new(());",
    "use tauri::AppHandle;",
    "impl EvolutionPolicy for X {}",
    # 泛型形态（规则 2 放宽后必须抓到）
    "impl<T> EvolutionPolicy for X {}",
    # group use 形态（规则 3 补洞后必须抓到）
    "use crate::evolution::{strategy, store};",
    # rand 随机 API 与 Rng 家族（规则 4 补洞后必须抓到）
    "let x = rand::random::<u8>();",
    "let mut g = rand::rng();",
    "let mut s = ThreadRng::default();",
    "let seed = OsRng::new();",
]

# 违例样例的抓取判定：与 RULES 里各禁入模式同源（新增模式必须同步进这里）
CAUGHT_RE = (
    r"\bstd::fs\b|chrono::Utc::now|thread_rng|\bMutex\b|tauri::"
    r"|impl\b[^;{]*?\bEvolutionPolicy\b"
    r"|evolution::strategy|evolution::\{[^}]*\bstrategy\b"
    r"|rand::(?:random|rng|seq|distributions)\b"
    r"|\b(?:Os|Small|Std|Thread)Rng\b"
)


def selftest() -> int:
    import tempfile
    ok = True
    with tempfile.TemporaryDirectory() as td:
        # 正例：SELFTEST_PASS 内容必须零违例
        probe = Path(td) / "strategy.rs"
        probe.write_text(SELFTEST_PASS, encoding="utf-8")
        raw = probe.read_text(encoding="utf-8")
        cleaned = strip_cfg_test_modules(strip_comments_and_strings(raw))
        if re.search(
            r"\bstd::fs\b|thread_rng|\bMutex\b|chrono::Utc::now"
            r"|rand::(?:random|rng|seq|distributions)\b|\b(?:Os|Small|Std|Thread)Rng\b",
            cleaned,
        ):
            print("✗ selftest：正例被误判违例（注释/字符串/原始串/测试模块剥离失效）", file=sys.stderr)
            ok = False
        # cfg(test) 剥离保换行：剥离前后行数一致，违例行号不整体偏移
        line_probe = (
            "#[cfg(test)]\nmod tests {\n    use std::fs;\n}\n"
            'let x = std::fs::read_to_string("p");\n'
        )
        stripped = strip_cfg_test_modules(strip_comments_and_strings(line_probe))
        if stripped.count("\n") != line_probe.count("\n"):
            print("✗ selftest：cfg(test) 剥离吞换行（违例行号会整体偏移）", file=sys.stderr)
            ok = False
        # 委托壳行级豁免：受认可形式放行、非认可 strategy 引用照抓
        # （样本须与白名单同源：批 2.2 后合法形态是 gate_decision/GateDecision）
        delegate_line = "let r = crate::evolution::strategy::gate_decision(a, b);"
        if not DELEGATE_ALLOW.search(delegate_line):
            print("✗ selftest：委托允许正则失效", file=sys.stderr)
            ok = False
        rogue = "use crate::evolution::strategy::store;"
        if DELEGATE_ALLOW.search(rogue):
            print("✗ selftest：非认可 strategy 引用未被排除", file=sys.stderr)
            ok = False
        # 反例：每条违例模式必须被抓到
        for i, sample in enumerate(SELFTEST_FAIL_SAMPLES):
            probe.write_text(sample, encoding="utf-8")
            raw = probe.read_text(encoding="utf-8")
            cleaned = strip_cfg_test_modules(strip_comments_and_strings(raw))
            caught = bool(re.search(CAUGHT_RE, cleaned))
            if not caught:
                print(f"✗ selftest：违例样例 {i} 未被抓到：{sample}", file=sys.stderr)
                ok = False
    if ok:
        print("✓ audit_evolution_layering selftest 通过")
        return 0
    return 1


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    if not EVO.exists():
        print(f"✗ evolution 目录不存在：{EVO}", file=sys.stderr)
        return 1
    violations = run_checks()
    if violations:
        print(f"✗ evolution 分层依赖方向守卫：{len(violations)} 处违例", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        print("  分层契约见 docs/EVOLUTION-LAYERING-BATCH-A.md §8", file=sys.stderr)
        return 1
    print("✓ evolution 分层依赖方向守卫通过（策略纯度/上下文边界/数据层方向/无随机源）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
