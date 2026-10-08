# 非程序员代码审计速查表

> 老板（编程小白）专用 · 项目：WMessage（Tauri 2 + React 19 + TypeScript）
> 适用人群：会跑命令、不读代码也能找 bug 的人
> 阅读时长：45 分钟（通读）/ 5 分钟（按章节查）

---

## 目录

- [§0 心法：审计的三个姿势](#§0-心法审计的三个姿势)
- [§1 工具准备](#§1-工具准备)
- [§2 ripgrep 速查](#§2-ripgrep-速查)
- [§3 找 bug 套路（按代码模式分类）](#§3-找-bug-套路按代码模式分类)
- [§4 何时问 AI + 怎么问](#§4-何时问-ai--怎么问)
- [§5 注释当假设（注释质量地图）](#§5-注释当假设注释质量地图)
- [§6 审计节奏模板（一天怎么过）](#§6-审计节奏模板一天怎么过)
- [§7 产出模板（怎么记 bug））](#§7-产出模板怎么记-bug)
- [§8 常见坑（小白最容易踩的）](#§8-常见坑小白最容易踩的)
- [§9 案例演练（真实 bug 还原）](#§9-案例演练真实-bug-还原)

---

## §0 心法：审计的三个姿势

**核心认知**：你**不需要懂 Rust / TypeScript**。你需要的是：

| 姿势 | 你做的 | 工具/帮手 |
|---|---|---|
| **跑（Run）** | 启动 app、点按钮、看现象 | wmessage app + 截图 |
| **搜（Search）** | 用 ripgrep 找可疑代码、统计模式 | ripgrep / grep |
| **问（Ask）** | 把可疑点丢给 Kimi/我：「这段在干嘛？」 | Kimi CLI / 我 |

**不做的**：
- ❌ 自己改代码
- ❌ 试图理解语法细节
- ❌ 试图"读懂"一个长函数

**做的**：
- ✅ 知道「这里有可疑点」就够了
- ✅ 把可疑点 + 复现步骤 写下来 → 让 Kimi 修

---

## §1 工具准备

### 1.1 macOS 自带（不用装）

```bash
cat <file>         # 看整个文件
less <file>        # 分页看（q 退出，/xxx 搜索）
find <dir> -name "*.rs"  # 找文件
grep -rn "关键字" <dir>  # 搜内容（旧 grep）
```

### 1.2 ripgrep（强烈推荐，比 grep 快 + 支持 .gitignore）

```bash
# 装：
brew install ripgrep

# 验证：
rg --version    # 应该是 ripgrep 14.x+
```

### 1.3 git 命令（审计必备）

```bash
git log --oneline -20                          # 最近 20 个 commit
git log --oneline --all -- <file>              # 某文件被改过几次
git log -p --follow <file> | head -100         # 某文件最近一次改了什么
git blame <file> | head -50                    # 谁什么时候写的某行
git show <commit-hash>                         # 看一个 commit 的详情
```

### 1.4 VSCode（看代码用）

- 装 Rust 扩展（rust-analyzer）→ `.rs` 文件能跳转定义
- 装 Tauri 扩展 → 看 `tauri.conf.json` / `capabilities/`
- 不要装 debugger（用不上）

### 1.5 不要装的

| 工具 | 为什么不要 |
|---|---|
| rustc / cargo | 你不编译代码，Kimi 编 |
| Node.js | 你不写前端，Kimi 写 |
| Docker | 项目不用 |
| Postgres / Redis | 项目用 SQLite，不是它们 |

---

## §2 ripgrep 速查

### 2.1 基础

```bash
# 搜内容（在所有文件里找含 "bot_chat" 的行）
rg "bot_chat" .

# 限定文件类型
rg "bot_chat" --type rust    # 只搜 .rs
rg "bot_chat" -t ts          # 只搜 .ts/.tsx

# 限定目录
rg "error" src-tauri/src/
rg "TODO" src/frontend/src/components/

# 显示行号（默认就有）
rg "unwrap" src-tauri/src/bot_py.rs
```

### 2.2 找可疑模式（直接抄）

| 目的 | 命令 |
|---|---|
| 找 `unwrap()` / `expect()`（panic 风险） | `rg "\.unwrap\(\)\|\.expect\(" src-tauri/src/` |
| 找 `.catch(() => {})`（吞错） | `rg "\.catch\(\(\) =>" src/frontend/src/` |
| 找 `panic!` | `rg "panic!\(" src-tauri/src/` |
| 找 `TODO` / `FIXME` | `rg "TODO\|FIXME\|XXX\|HACK" src/` |
| 找硬编码密钥 | `rg -i "api[_-]?key\s*[:=]\s*['\"]" src/` |
| 找空字符串兜底 | `rg 'msg\s*\|\|\s*"未知' src/frontend/src/` |
| 找 magic number | `rg "[0-9]{3,}" src-tauri/src/bot_py.rs` |
| 找 `console.log`（残留调试） | `rg "console\.log\|console\.warn" src/frontend/src/` |
| 找 `eprintln!`（应该改审计日志的） | `rg "eprintln!\|dbg!" src-tauri/src/` |
| 找 `// XXX` / 警告 | `rg "//\s*(XXX|WARNING|⚠|FIXME|HACK)" src/` |

### 2.3 统计（看分布）

```bash
# 哪个文件最大（找可疑大文件）
rg --files src-tauri/src/ | xargs wc -l | sort -rn | head -10

# 某个函数被调用几次
rg "bot_chat\(" --no-filename | wc -l

# 某字符串全局出现次数
rg "TODO" src/ --count
```

### 2.4 上下文（看周围 3 行）

```bash
# 默认显示匹配行的前后 3 行
rg -C 3 "drop_file_on_task" src/

# 只显示前/后 N 行
rg -B 2 -A 5 "ExecGuard" src/
```

### 2.5 排除（不看测试 / target）

```bash
# rg 默认遵循 .gitignore，自动排除 target/ node_modules/
# 显式排除：
rg "TODO" --type-add 'web:*.{ts,tsx}' -t web -g '!*.test.*'
# 上面的：找所有 .ts/.tsx 的 TODO，但排除 .test.ts

# 排除路径
rg "panic" src-tauri/src/ -g '!src-tauri/tests/**'

# 多模式（OR）
rg -e "unwrap" -e "expect" src-tauri/src/bot_chat.rs

# 反向：找不含某字符串的文件
rg -L "test" src-tauri/src/bot.rs
```

### 2.6 文件名搜索（不用看内容）

```bash
# 找文件名含 "widget" 的所有文件
rg --files src/ -g '*widget*'

# 找刚改过的 Rust 文件
rg --files src-tauri/src/ --type rust | xargs -I {} sh -c 'echo "=== {} ===" && git log -1 --format="%h %s" -- {}'
```

---

## §3 找 bug 套路（按代码模式分类）

### 3.1 「panic 风险」

**模式**：`.unwrap()` / `.expect("...")` / `panic!("...")`

**为什么是 bug**：
- 任何一行 panic = app 闪退
- 用户数据可能在写入中丢失

**怎么找**：
```bash
rg "\.unwrap\(\)" src-tauri/src/
rg "\.expect\(" src-tauri/src/
rg "panic!\(" src-tauri/src/
```

**怎么判定严重度**：
| 出现位置 | 严重度 |
|---|---|
| 测试代码 `#[cfg(test)]` | ✅ 正常 |
| 启动期（lib.rs run 闭包前 100 行） | ⚠️ 可接受（启动失败 = 进程没起来） |
| 业务逻辑（bot_chat.rs / db.rs / api_handlers.rs） | 🔴 P0 |
| 错误恢复路径 | 🔴 P0（恢复不了比不恢复更糟） |

**怎么处理**：
- 发现业务代码有 unwrap → 截图 + 贴代码 → 问 Kimi：「这个 unwrap 会触发吗？怎么改 `?`？」

### 3.2 「空 catch / 错误吞掉」

**模式**：
- Rust：`if let Err(_) = ... { }` 或 `let _ = something_that_can_fail()`
- TS：`.catch(() => {})` / `.catch(() => null)` / `try { ... } catch {}`

**为什么是 bug**：
- 失败没人知道 = 用户以为成功实际没成
- 数据丢失无法追责

**怎么找**：
```bash
# Rust
rg "let _\s*=" src-tauri/src/ | grep -i "fs::\|invoke\|db_\|api_" | head
rg "if let Err" src-tauri/src/

# TS
rg "\.catch\(\(\) =>" src/frontend/src/
rg "\.catch\(\(_\)" src/frontend/src/
rg "catch\s*\(\s*[a-z_]*\)\s*\{\s*\}" src/frontend/src/
```

**判定**：
- 是「明知会失败但允许失败」（如轮询超时） → ✅ 合理
- 是「写操作失败被吞」 → 🔴 P0

### 3.3 「TODO / FIXME」

**模式**：`// TODO` / `// FIXME` / `// XXX` / `// HACK` / `// ⚠`

**为什么是 bug**：
- 作者自己承认没做完
- 可能是已知未修的 bug

**怎么找**：
```bash
rg "TODO\|FIXME\|XXX\|HACK" src/ -C 1
```

**判定**：
- 有具体 issue 链接 / commit hash → 跟踪
- 只是「todo: 这个函数要重构」 → 排期
- 「todo: 处理 X 失败」 → 🔴 必查（X 是哪个？处理了吗？）

### 3.4 「硬编码 / magic number」

**模式**：`300s` / `5MB` / `120s` / `port: 4763` / `path: "/Users/..."`

**怎么找**：
```bash
# 大数字（秒、字节）
rg "[0-9]{2,}" src-tauri/src/ | rg "sec\|bytes\|MB\|timeout\|limit" 

# 端口号
rg ": 4763\|4763\b" src/

# 硬编码路径
rg '"/Users/\|"/home/\|"C:\\\\"' src/
```

**判定**：
- 有 const + 注释解释 → ✅ 合理
- 散落在函数体里、无注释 → ⚠️ 问 Kimi「这个值从哪来？改了会怎样？」

### 3.5 「权限 / 越权」

**关键文件**：
- `src-tauri/capabilities/default.json` —— Tauri 权限清单
- `src-tauri/src/api_auth.rs` —— API token 验证
- `src-tauri/src/middleware.rs` —— 中间件注册
- `src-tauri/src/tool_guard.rs` —— 工具白/黑名单

**怎么查**：
```bash
# 权限 scope（看有没有过宽）
cat src-tauri/capabilities/default.json

# 工具白/黑名单（看有没有遗漏）
rg "ATOMIC_BLACKLIST\|ATOMIC_WHITELIST" src-tauri/src/tool_guard.rs -A 20

# token 比较（看是否恒定时间）
rg "verify_bearer\|ct_eq" src-tauri/src/api_auth.rs -A 5
```

**判定**：
- `opener:allow-open-path` scope 含 ` `**` → 🔴 P0（可打开任何路径）
- 黑名单 ≤2 个工具 → 合理
- 黑名单里没 `delete_task` 但白名单里有 → 🔴 P0
- token 比较用 `==` 而非 `ct_eq` → 🔴 P0

### 3.6 「数据持久化」

**关键文件**：`src-tauri/src/db.rs` / `migration.rs`

**怎么查**：
```bash
# 找所有写操作
rg "fn.*upsert\|fn.*update\|fn.*delete\|fn.*insert" src-tauri/src/db.rs -A 3

# 找事务包裹
rg "conn\.transaction\|tx\.commit" src-tauri/src/

# 找原子写
rg "atomic_write\|fs::rename" src-tauri/src/

# 找没事务的循环写
rg "for .* in .* \{[\s\S]*?stmt\.execute" src-tauri/src/db.rs | head
```

**判定**：
- 单条 `INSERT`/`UPDATE` → ✅ OK
- 循环 `for ... execute(...)` 没 `tx.commit()` → 🔴 P0（中途失败会留半截数据）

### 3.7 「并发 / 锁」

**怎么查**：
```bash
# 找 Mutex / RwLock
rg "Mutex::new\|RwLock::new\|parking_lot::Mutex" src-tauri/src/

# 找全局 static（潜在 race）
rg "static [A-Z]" src-tauri/src/

# 找 unwrap on Mutex lock（中毒风险）
rg "\.lock\(\)\.unwrap" src-tauri/src/
```

**判定**：
- `static MUTEX: Mutex<...> = Mutex::new(...)` → ✅ 标准模式
- `static UNSAFE_MUT` / `static CELL` → 🔴 重点查
- `.lock().unwrap()` → ⚠️ 中毒会 panic，问 Kimi

### 3.8 「前端空 catch」

**怎么查**：
```bash
rg "\.catch\(\(\) =>\s*(\{\s*\}|null)" src/frontend/src/
rg "try\s*\{[^}]+\}\s*catch\s*\(\s*[a-z_]*\s*\)\s*\{\s*\}" src/frontend/src/
```

**判定**：
- 已知可失败的旁路（emit / openUrl） → ⚠️ 接受
- 写操作（upsertTasks / saveRules） → 🔴 P0

### 3.9 「错误处理兜底」

**怎么查**：
```bash
# 空字符串 / 默认值兜底
rg 'msg\s*\|\|\s*"\|\s*e\.code\s*\|\|\s*"' src/frontend/src/

# try-catch 包 try 后什么都不做
rg "catch.+\{[\s\n]*\}" src/frontend/src/
```

### 3.10 「敏感信息泄露」

**怎么查**：
```bash
# API key / token 出现在日志
rg "audit_event!\|write_event" src-tauri/src/audit.rs | rg "key\|token\|password\|secret"
rg "console\.log\|eprintln" src/

# 用户输入未 escape 进日志
rg "audit_event.*\{" src/ -A 1 | rg "task\.\|title\|preview" | head
```

---

## §4 何时问 AI + 怎么问

### 4.1 何时问

| 触发条件 | 问什么 | 谁来问 |
|---|---|---|
| 看到一段代码 > 10 行不懂 | 「这段在干嘛？input/output 是啥？」 | Kimi |
| 看到可疑模式（见 §3）但不知严重度 | 「这个 unwrap / catch / TODO 实际会不会触发？」 | Kimi |
| 功能行为与注释不符 | 「注释说 X，代码做的是 Y，是不是 bug？」 | Kimi |
| 想做改动但不知影响面 | 「改这里会影响哪些地方？调用方有哪些？」 | 我（更稳） |
| 跨文件牵连 | 「A 改了会不会影响 B、C？」 | 我 |
| 设计意图问题 | 「为什么要这样设计？有更好方案吗？」 | 我 |

### 4.2 怎么问（提问模板）

**好问题模板**：
```
我在 [文件路径] 看到 [可疑代码片段]。

我的疑问是：
- [具体问题 1]
- [具体问题 2]

我的猜测是：
- [你的判断]

请帮我：
- 解释这段在干嘛（不要讲语法）
- 评估严重度（P0/P1/P2/P3）
- 如果是 bug，建议怎么修（一句话方向即可）
```

**示例**：
```
我在 src-tauri/src/bot_py.rs:1360 看到：
if !py_get_enabled(app.clone()) {
    return Err("Python 编程未开启：...".into());
}

我的疑问是：
- 这条错误用户会看到吗？
- 如果 Python 未装（vs 未开启），错误一样吗？
- 错误 code 是什么？前端能不能拿到？

请帮我：
- 解释这是干啥的
- 评估是不是 bug
```

### 4.3 不要问 AI 什么

| 别问 | 为什么 |
|---|---|
| 「帮我改一下这段代码」 | 你不审，让 Kimi 改；先定位 bug 再改 |
| 「这段语法对吗」 | 编译会告诉你，不用问 AI |
| 「有没有更好的实现」 | 跑题，先把现有问题找完 |
| 「这是不是最佳实践」 | 主观题，浪费时间 |

### 4.4 我 vs Kimi 的分工

| 任务 | 适合谁 | 原因 |
|---|---|---|
| 解释代码（5-30 行） | Kimi | 便宜快速 |
| 找已知 bug | Kimi | 模式匹配强 |
| 跨模块影响分析 | 我 | 需要读多文件 + 记住约束 |
| 设计意图 / 拍板 | 我 | 需要项目背景 |
| 写测试用例 | Kimi | 模板化 |
| 重构方案 | 我 | 涉及 forbidden 文件 / 约束 |

---

## §5 注释当假设（注释质量地图）

### 5.1 wmessage 注释可信度分级

| 区域 | 可信度 | 备注 |
|---|---|---|
| `src-tauri/src/bot_chat.rs` | ⭐⭐⭐⭐ | Kimi era，注释密集 |
| `src-tauri/src/bot_skills.rs` | ⭐⭐⭐⭐ | 决策注释多 |
| `src-tauri/src/audit.rs` | ⭐⭐⭐⭐⭐ | P0 事件命名空间有显式声明 |
| `src-tauri/src/middleware.rs` | ⭐⭐⭐⭐ | middleware 钩子意图清晰 |
| `src-tauri/src/error.rs` | ⭐⭐⭐⭐⭐ | 注释是 spec |
| `src-tauri/src/api_*.rs` | ⭐⭐⭐⭐ | Phase 7 改的多 |
| `src-tauri/src/bot_py.rs` | ⭐⭐⭐⭐ | 沙箱熔断注释好 |
| `src-tauri/src/db.rs` | ⭐⭐⭐ | 部分旧注释 stale |
| `src-tauri/src/migration.rs` | ⭐⭐⭐ | journal 部分可信 |
| `src/components/*.tsx` | ⭐⭐ | 自描述注释多，**易 stale** |
| `src/components/TaskCardContent.tsx` | ⭐⭐⭐ | 老板拍板的多，注释可信 |

### 5.2 读注释的三个技巧

1. **看时间戳**：注释离 commit 越近越可信
   ```bash
   git log -p --follow src/components/TodoCard.tsx | head -50
   ```
2. **看 blame**：注释所在行的 `git blame` 找作者
3. **看 PR/commit body**：commit message 里的「为什么」比代码注释更准

### 5.3 「注释是谎」的三种迹象

- ❌ 注释提到「X 已废弃 / Y 不再需要」→ 但代码里 X/Y 还在 → stale
- ❌ 注释说「安全 / 不会 panic」→ 代码里有 unwrap 或 catch 空 → 矛盾
- ❌ 注释列了 3 种 case → 代码只 if else 处理 2 种 → 漏了

---

## §6 审计节奏模板（一天怎么过）

### 6.1 一天 2-3 小时有效审计节奏

| 时间 | 工作 | 产出 |
|---|---|---|
| **0:00 - 0:10** | 选今天要审的 1-2 个功能 | 任务清单 |
| **0:10 - 0:20** | 读 MANUAL-ACCEPTANCE 该功能条目 | 「期望」笔记 |
| **0:20 - 0:40** | ripgrep 找代码 + 读关键函数注释 | 「假设」笔记 |
| **0:40 - 1:20** | 手动跑功能，验证假设 vs 实际 | 标 ✅/❌ |
| **1:20 - 1:50** | ❌ 项 → 写 bug 报告（含复现 + 截图） | bug 清单 +1 |
| **1:50 - 2:00** | 问 Kimi/我处理 1-2 个最关键的疑点 | 解惑 |

### 6.2 一天找 1-3 个 bug 是正常速度

不要追求数量。**找 1 个真实 bug 比找 10 个似是而非的「可疑点」值钱**。

---

## §7 产出模板（怎么记 bug）

### 7.1 bug 报告模板

```markdown
## Bug: <一句话描述>

**严重度**：P0 / P1 / P2 / P3
**功能**：<对应 MANUAL-ACCEPTANCE 第 X 项>
**发现时间**：2026-08-19 21:30
**复现步骤**：
1. 打开 wmessage
2. <具体操作>
3. <具体操作>
4. 观察：<错误现象>

**期望**：<应该发生什么>
**实际**：<实际发生什么>
**截图**：<附图>

**代码线索**（如有）：
- 文件：src-tauri/src/bot_chat.rs:314
- 代码片段：`if !bot_get_enabled(...) { return Err(...) }`
- 注释说：xxx
- 我怀疑：xxx

**问 Kimi / 我的问题**：
- 这个问题是 bug 还是设计如此？
- 严重度应该是 P0/P1/P2？
- 建议修法（一句话方向即可）
```

### 7.2 bug 分级标准

| 等级 | 含义 | 例子 |
|---|---|---|
| **P0** | 数据丢失 / 安全漏洞 / 闪退 | DB 裸 unwrap、token 明文 log、panic 在业务路径 |
| **P1** | 功能不工作 | 按钮点了没反应、文件操作失败 |
| **P2** | 边界 / 体验 | 长输入截断丢失、错误文案不友好 |
| **P3** | 代码质量 | 重复代码、命名不清、过长函数 |

---

## §8 常见坑（小白最容易踩的）

### 8.1 不要 ❌

| 坑 | 为什么错 |
|---|---|
| 自己改代码 | 编译 / 测试 / git 流程你不熟，会引入新 bug |
| 用 grep 不用 rg | 慢 10 倍，看不到 .gitignore 排除 |
| 看一长段代码试图理解 | 浪费时间，只看函数级注释 + 调用方就够 |
| 看到 TODO 就报 bug | TODO ≠ bug，可能是有意保留 |
| 看到 unwrap 就报 bug | 测试代码 / 启动期 unwrap 是正常的 |
| 跨大版本审计（前 6 个月代码 + 现在代码） | 质量不一，会被旧注释带偏 |
| 一次问 AI 1000 行 | AI context 超载，回答质量暴跌 |
| 不复现就报告 | 可能你操作错了，先确认复现 3 次 |

### 8.2 要做 ✅

| 事 | 为什么对 |
|---|---|
| 每次只审 1-2 个功能 | 保持专注 |
| 复现 3 次再报 | 排除偶发 |
| 注释 + 行为 双验证 | 防注释 stale |
| 把可疑点丢给 AI 前先 grep 一下 | AI 给的建议常常基于 grep 你也能找到 |
| 写 bug 报告时贴代码片段 + commit hash | 修的人不用从头看 |
| 怀疑时问「这个 unwrap 实际会触发吗？」 | 让 AI 给严重度，别自己判断 |
| 完成一天后 commit bug 报告 | 防止丢笔记 |

---

## §9 案例演练（真实 bug 还原）

### 9.1 案例 1：挂件折叠丢流式回复（2026-08-19 老板亲自发现的）

**症状**：与机器人聊天，未回复完成 → 挂件自动折叠 → 重新展开 → 聊天回复消失

**小白审计步骤**：
1. 读 `MANUAL-ACCEPTANCE.md` 五.「挂件折叠不丢聊天」（这条**当时还不存在** → 说明这是个真新 bug）→ 期望：「折叠不应丢数据」
2. `rg "ChatPanel\|折叠\|mouseLeave" src/components/WidgetApp.tsx`
3. 看 WidgetApp.tsx 449 行：
   ```tsx
   onMouseLeave={collapse}
   ```
4. 问 Kimi：「`onMouseLeave={collapse}` 会让 ChatPanel 整个 unmount 吗？」
5. Kimi 答：会，因为 `{!expanded ? 触发条 : 展开面板}` 是三元分支
6. 进一步：跑一遍功能 → 确认折叠丢消息
7. 报 bug → Kimi 修 → commit 8c6c9d7

**审计员学到的**：
- 一个 `onMouseLeave` 也能藏 bug
- 「应该丢」和「实际丢」是两回事
- 注释承诺的是期望，行为给的是事实

### 9.2 案例 2：unbind File 但 folder 已绑（潜在 bug，待审）

**audit 步骤**：
1. `rg "fileIsDir\|isDir" src/components/TodoCard.tsx | head -20`
2. 看 `pickFolder` 函数（TodoCard.tsx:131）→ 注释说「已绑文件夹则弹提示」
3. 问：「如果 task.files 已经有 1 个文件，用户点 📁 绑文件夹 → 代码怎么处理？」
4. 看代码：直接 `onUpdate(task.id, { filePath: selected, fileIsDir: true })` → **覆盖** 文件！
5. 这就是 bug：注释承诺了行为，代码没兑现

**审计员学到的**：
- 「承诺 vs 兑现」是核心审计姿势
- 多文件改造（commit f6fcc17）后老 bug 可能继承下来

### 9.3 案例 3：SSE writer 泄漏（已修，看历史）

**审计步骤**：
1. `git log --all --oneline -- src-tauri/src/api_handlers.rs | head`
2. 看到 G1 commit（Phase 6c）：`fix(api): G1 api_stop/rotate_token 通知 SSE writer 线程`
3. 问 Kimi：「修前是什么 bug？什么场景触发？」
4. Kimi 答：「关 API 后 SSE writer 线程没 join，进程不退出」
5. 看 commit 的 diff → 学到一个套路：资源生命周期问题看 `Drop`/`join`/`stop` 是否齐全

---

## 附录：常用快捷命令

```bash
# 进入项目
cd /Users/renshi/Projects/wmessage

# 今天的 commit
git log --since="today" --oneline

# 昨天的 commit
git log --since="yesterday" --until="today" --oneline

# 看一个文件被谁动过
git log --oneline --all -- src/components/ChatPanel.tsx

# 全局找所有 unwrap / expect
rg "\.unwrap\(\)|\.expect\(" src-tauri/src/ | wc -l

# 找含文件大小（找可疑大文件）
find src-tauri/src -name "*.rs" -exec wc -l {} \; | sort -rn | head -10

# 跑测试（别自己跑，让 Kimi 跑）
# cargo test --lib
# npx test -- --run
```

---

*文档由小九（OpenClaw Agent）生成 · 2026-08-19*
*对应 commit `47b14ca`（任务卡多文件绑定改造后）*
*适用版本：WMessage v1.0.0*

**使用建议**：
1. 第一天：通读一遍（45 分钟）
2. 之后：按 §6 节奏 + §7 模板用
3. 卡住：翻 §3 找套路 / 翻 §4 问 AI
4. 一周后：回看你的 bug 报告，提炼常见模式 → 加进你自己的 cheatsheet