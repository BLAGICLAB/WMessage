#!/usr/bin/env python3
"""一次性脚本：为 src/**/*.tsx 中的硬编码颜色类批量添加 dark: 变体。"""
import re, pathlib, sys

ROOT = pathlib.Path("/Users/renshi/Projects/wmessage/src")

# (模式, 替换) —— 负向环视避免误匹配 hover:/dark: 前缀内的 token
REPL = [
    # 基础文字色
    (r"(?<![\w:])text-gray-900(?![\w-])", "text-gray-900 dark:text-gray-100"),
    (r"(?<![\w:])text-gray-800(?![\w-])", "text-gray-800 dark:text-gray-100"),
    (r"(?<![\w:])text-gray-700(?![\w-])", "text-gray-700 dark:text-gray-200"),
    (r"(?<![\w:])text-gray-600(?![\w-])", "text-gray-600 dark:text-gray-300"),
    (r"(?<![\w:])text-gray-500(?![\w-])", "text-gray-500 dark:text-gray-400"),
    (r"(?<![\w:])text-gray-400(?![\w-])", "text-gray-400 dark:text-gray-500"),
    (r"(?<![\w:])text-gray-300(?![\w-])", "text-gray-300 dark:text-gray-600"),
    (r"(?<![\w:])text-green-600(?![\w-])", "text-green-600 dark:text-green-400"),
    (r"(?<![\w:])text-red-500(?![\w-])", "text-red-500 dark:text-red-400"),
    (r"(?<![\w:])bg-white/70(?![\w-])", "bg-white/70 dark:bg-white/10"),
    (r"(?<![\w:])bg-white/60(?![\w-])", "bg-white/60 dark:bg-white/10"),
    (r"(?<![\w:])accent-gray-500(?![\w-])", "accent-gray-500 dark:accent-gray-400"),
    # hover 变体
    (r"(?<![\w-])hover:text-gray-800(?![\w-])", "hover:text-gray-800 dark:hover:text-gray-100"),
    (r"(?<![\w-])hover:text-gray-700(?![\w-])", "hover:text-gray-700 dark:hover:text-gray-200"),
    (r"(?<![\w-])hover:text-gray-600(?![\w-])", "hover:text-gray-600 dark:hover:text-gray-200"),
    (r"(?<![\w-])hover:text-gray-500(?![\w-])", "hover:text-gray-500 dark:hover:text-gray-300"),
    (r"(?<![\w-])hover:text-gray-400(?![\w-])", "hover:text-gray-400 dark:hover:text-gray-500"),
    (r"(?<![\w-])hover:text-red-500(?![\w-])", "hover:text-red-500 dark:hover:text-red-400"),
    (r"(?<![\w-])hover:bg-black/5(?![\w-])", "hover:bg-black/5 dark:hover:bg-white/10"),
    (r"(?<![\w-])hover:bg-black/10(?![\w-])", "hover:bg-black/10 dark:hover:bg-white/10"),
]

count = 0
for f in sorted(ROOT.rglob("*.tsx")):
    text = f.read_text()
    orig = text
    for pat, rep in REPL:
        text = re.sub(pat, rep, text)
    if text != orig:
        f.write_text(text)
        count += 1
        print(f"updated {f.relative_to(ROOT)}")
print(f"\n{count} files updated")
