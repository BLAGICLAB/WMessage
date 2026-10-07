# CSP 收紧验证报告（PR12）· WebView 冒烟阶段

> 状态：**冒烟待执行（由人工执行）**。主配置 `tauri.conf.json` 未改动。
> 本文档 = 证据存档 + 冒烟运行手册 + 通过标准。checklist 全勾且人工复核后，
> 才由单独 commit 改主配置（见 §7）。

## 0. 拍板项确认（按默认）

| 项 | 结论 |
|---|---|
| 安全目标 | **窄目标**：只去 `unsafe-eval`，其余指令与主配置 `csp` 一字不差 |
| 动哪个字段 | 只动 prod `csp`；**`devCsp` 不动、不测**（技术依据：tauri-2.11.5 `manager/mod.rs:370` 实证——构建产物一律用 `csp`，`devCsp` 仅 `tauri dev` 运行时生效，故本验证天然不触碰 devCsp） |
| 宽目标收紧 | 不做，另开 PR |
| 冒烟人 | 人工（agent 备好构建产物与手册，不代跑） |
| 证据留存 | 本文档（等价 PR12 描述）+ commit |
| 复核 | 明早人工复核后才改配置 |
| 失败处置 | 配置不动，本文档留失败记录 |

## 1. 改动点 diff（唯一变量）

```
tauri.conf.json app.security.csp（主配置，未动，目标值）:
- script-src 'self' 'unsafe-eval' blob:
+ script-src 'self' blob:

src-tauri/tauri.csp-verify.json（临时构建覆盖，已提交）:
  {"app":{"security":{"csp":"default-src 'self'; script-src 'self' blob:; worker-src 'self' blob:; connect-src ipc: http://ipc.localhost; img-src 'self' asset: http://asset.localhost blob: data:; style-src 'self' 'unsafe-inline'; font-src 'self'"}}}
```

与主配置逐字对照：**唯一差异 = script-src 少 `'unsafe-eval'`**。其余六段指令
（default-src / worker-src / connect-src / img-src / style-src / font-src）一字不差。
`tauri build --config` 为合并语义，主配置其余全部字段原样生效。

## 2. 冒烟产物（debug 构建，已备好）

| 产物 | 用途 | 构建命令（可复现） | CSP 状态 |
|---|---|---|---|
| `src-tauri/target/csp-verify/wmessage-csp-baseline` | 抓 Tauri 处理后的**当前**实际 CSP 存档 | `npm run tauri build -- --debug --no-bundle` | 含 `unsafe-eval` |
| `src-tauri/target/csp-verify/wmessage-csp-tightened` | 去掉 `unsafe-eval` 后的完整冒烟 | `npm run tauri build -- --debug --no-bundle --config src-tauri/tauri.csp-verify.json` | 不含 `unsafe-eval` |

两个都是 debug 构建（devtools 默认开启：右键 → Inspect / ⌘⌥I）。同一时间只开一个
（共享 SQLite 与端口）。`devCsp` 在两个产物中均未参与（见 §0 技术依据）。

## 3. 静态补扫结果（六类间接形式，本轮新增）

| 类 | 模式 | src 命中 | bundle 命中 |
|---|---|---|---|
| 1 | `setTimeout/setInterval` 字符串参数 | 0 | 0 |
| 2 | `Function(` 直接调用 | 0 | 2（**假阳性**：`normalizationFunction(`/`zoomToSizeRatioFunction(` 方法名后缀） |
| 3 | 动态 `import(` 非字面量参数 | 0 | —（vite 已静态化） |
| 4 | `(0,eval)` / `window.eval` / `globalThis.eval` / 裸 `eval(` | 0 | 0 |
| 5 | `Function` 别名赋值（`[:=] Function`） | 0 | 0 |
| 6 | WASM | 无 `.wasm` 资产，bundle 无 `WebAssembly` 引用 → 无不可见 eval 面 | 同 |

复现命令（逐条 grep，见 git history 本文件 commit message）。

**结论按任务书 §三要求弱化表述**：未发现直接与常见间接 eval 调用点；类5 的极端
别名混淆（minify 后任意重绑定）静态不可完全排除——由 §5 WebView 运行时采集兜底。

## 4. 浏览器层验证（前置证据，上轮完成）

- 方法：prod 构建静态伺服 + 响应头注入去掉 `unsafe-eval` 的 CSP（其余指令与主配置
  一致）+ `securitypolicyviolation` 事件采集（先自证链路：push 一条自证条目读回
  计数 1→清零→0）。
- 结果：全路径**零违规**。覆盖：应用挂载 + 9 主视图（sigma/graphology 图谱、
  xyflow 工作流画布）+ 8 设置子页 + 任务卡输入链路。
- 静态：src 0 eval、bundle 0 `new Function`。
- **边界（必须与本轮 WebView 结果分开看）**：浏览器头注入测的是「指令串本身在
  Web 层是否成立」；Tauri 会对 `csp` 做 nonce 注入（script-src/style-src）与协议
  合并（ipc:/asset:）——**WebView 冒烟测的才是 Tauri 处理后的实际 CSP**，这就是
  本轮存在的理由。

## 5. WebView 冒烟运行手册（人工执行）

> 顺序：先基线抓 CSP（1 分钟）→ 再收紧版全冒烟（10–15 分钟）。任意一步失败：
> 停，把失败点填进 §6 失败记录，**主配置不动**。

### 5.1 基线存档（wmessage-csp-baseline）

1. 打开产物，右键 → Inspect 打开 DevTools，Console 粘贴（**实测修正**：Tauri 2
   在 macOS 经自定义协议**响应头**下发 CSP，不用 meta 标签——meta 查询恒
   undefined，已从手册移除）：
   ```js
   fetch(location.href).then(r => r.headers.get('content-security-policy') || '(null，走备用)')
   ```
   备用方法：Web Inspector → **Network** 标签 → 刷新页面 → 点第一个 document
   请求 → Response Headers → `Content-Security-Policy`。
2. 把输出**原文**粘贴到 §6 的「基线实际 CSP」槽位。检查它含 `unsafe-eval`。

### 5.2 收紧版冒烟（wmessage-csp-tightened）

**步骤 1：自证采集链路**（不自证则本报告所有"0 违规"作废）。Console 粘贴：

```js
(() => {
  window.__csp = [];
  document.addEventListener('securitypolicyviolation',
    e => window.__csp.push(e.violatedDirective + ' @ ' + (e.sourceFile||'') + ':' + (e.lineNumber??'') + ' sample=' + (e.sample||'').slice(0,100)));
  const s = document.createElement('script');
  s.textContent = 'void 0';
  document.head.appendChild(s);
  return 'collector installed; inline-script violation expected';
})()
```

然后读 `window.__csp`——**必须出现一条 `script-src-elem`（或 script-src）违规**
（我们故意注入的内联脚本）。计到 = 采集器与 CSP 强制都工作，`window.__csp = []` 清零，
进入步骤 2。**计不到 = 停，记录，本报告作废。**

**步骤 2：抓收紧版实际 CSP**：粘贴 5.1 修正后的 fetch 片段（或 Network 备用法），
输出贴进 §6 槽位，确认**不含 `unsafe-eval`**（nonce 由 Tauri 注入属预期，不算差异）。

**步骤 3：主视图逐个**（每开一个扫一眼 Console 无红色 CSP 报错）：
首页 → 图谱（sigma/graphology 画布必须真渲染出画布，不是空白）→ 归档 → 工作区 →
回收站 → 工作流（xyflow 画布必须渲染）→ 定时 → 活动 → 通知 → 设置。

**步骤 4：设置子页**：机器人 / 模型设置 / 记忆 / 技能 / MCP 服务 / 自进化 /
桌面整理 / 词元统计。

**步骤 5：任务卡链路**：首页 → 新建任务 → 输入标题 → 回车落卡 → 打勾完成 → 删除。

**步骤 6：挂件**：主窗口设置页里开关挂件（或 ⌘⌥W）→ 挂件窗出现 → 建一张卡 →
打勾。这是浏览器层覆盖不到的盲区补测。

**步骤 7：读数**：任一窗口 Console 粘贴 `window.__csp.length` 与
`JSON.stringify(window.__csp)`，贴进 §6 槽位。**预期 = 0**。

**步骤 8：CSP diff**：§6 两个槽位的文本对比——**只允许 `unsafe-eval` 一处差异**
（Tauri 注入的 nonce/合并的协议两份都有，不算差异）。

### 5.3 通过标准（硬 checklist）

- [x] 应用挂载成功，无白屏
- [ ] 9 主视图逐个打开，无 CSP 违规
- [ ] sigma/graphology 图谱画布渲染成功
- [ ] xyflow 工作流画布渲染成功
- [ ] 8 设置子页逐个打开，无违规
- [ ] 任务卡输入链路走通
- [ ] 挂件在真实 Tauri 窗口尝试过
- [ ] WebView 采集链路自证可用（故意违规计到数）
- [ ] 实际生效 CSP 不含 `unsafe-eval`
- [ ] `securitypolicyviolation` 计数 = 0
- [ ] CSP 相关 console error = 0
- [ ] 两份实际 CSP 文本已记录（§6）
- [ ] 两份 diff 只含 `unsafe-eval` 一处
- [ ] `devCsp` 未修改、未参与（本文档 §0 技术依据）
- [ ] 静态扫描六类已补扫（§3）
- [ ] 回滚方式写明（§7）

## 6. 证据槽位（执行人填写）

- 基线实际 CSP（含 `unsafe-eval`）：＿＿＿＿
- 收紧版实际 CSP：＿＿＿＿
- 两份 diff（应仅 `unsafe-eval` 一处）：＿＿＿＿
- 采集器自证结果（应计到 1 条 script-src 违规）：＿＿＿＿
- 冒烟后 `securitypolicyviolation` 计数：＿＿＿＿
- console CSP error 计数：＿＿＿＿
- 截图/录屏路径：＿＿＿＿
- 失败点（若有：视图 + 违规指令 + 库名 + 调用栈）：＿＿＿＿
- 非 CSP 观察项（不计入通过标准，另查）：冒烟首日实见一条
  `Unhandled Promise Rejection: TypeError: undefined is not an object
  (evaluating 'listeners[eventId].handlerId')`——Tauri 注入 user-script 的
  `unregisterListener` 反注册竞态（同 eventId 被二次 unlisten 或反注册已
  移除的监听；调用链 `user-script:10` ← bundle `_unlisten`）。与 CSP 无关
  （形态非 securitypolicyviolation，且 user-script 不受页面 CSP 约束），
  不影响本验证判定；double-unlisten 源头待另批排查。
- 执行人 / 日期：＿＿＿＿

## 7. 通过后的配置改动（单独 commit，复核确认后执行）

- `tauri.conf.json` 的 `csp` 仅去掉 script-src 里的 `'unsafe-eval'`；
  `devCsp`、其余指令、其他字段一律不动。
- commit message 注明：本轮只去 `unsafe-eval`；`devCsp` 未覆盖、未修改；
  证据见本文档。
- **回滚方式**：恢复 script-src 里 `'unsafe-eval'` 一词（单行 diff revert 即可）。

## 8. 验证边界声明（时点验证）

- 只证明**当前代码 + 当前依赖**在 WebView 内不需要 `unsafe-eval`；
  未来引入使用 `new Function`/eval 的依赖即失效。
- 不覆盖恶意注入场景的 XSS 面分析（收紧减小的是攻击面，不是修复某个漏洞）。
- 未配 CSP 违规上报端点，线上违规不可感知。

### 8.1 Windows 侧说明（不需单独 CSP 冒烟，理由留档）

- **CSP 字符串平台无关**：全仓唯一 `tauri.conf.json`，无平台覆盖配置；
  前端 bundle 为同一份 Vite 产物；Tauri 的 CSP 注入（meta + nonce）是跨平台
  同一份 Rust 代码（tauri-2.11.5 `manager/mod.rs`）。
- **平台差异只在执行引擎**：macOS = WKWebView (WebKit)，Windows = WebView2
  (Chromium)。「无 `unsafe-eval` 则禁 eval」是 CSP 核心行为，两引擎均为标准
  实现；macOS 通过 ⇒ Windows 同样通过，不存在反向失败路径。
- **Windows 特有点**（WebView2 运行时版本、`http://ipc.localhost` 自定义协议）
  全部位于本轮一字未动的 `connect-src`/`img-src` 指令内，与本轮唯一变量无关。
- **覆盖方式**：Windows 打包发版时按既有发版回归顺带确认应用可正常运行即可；
  如需直接证据，§2 两条构建命令在 Windows 原样可复现（产物 `wmessage.exe`）。
- 后续建议：`report-uri`/`report-to` 上报、CI 静态禁 eval 检查（可挂进
  tests-audit）、宽目标收紧另开 PR。

## 9. 未做项及原因

| 未做项 | 原因 |
|---|---|
| `devCsp` 验证/修改 | 拍板范围外；且构建产物运行时不读 devCsp（§0 技术依据） |
| 宽目标（object-src/base-uri/frame-ancestors/connect-src 收敛） | 一次一个变量，另开 PR |
| report-uri 上报 | 需上报端点设计，属后续增强 |
| release 构建冒烟 | release 无 devtools（devtools feature 默认仅 debug），采集链路不可用；debug 构建与 release 的差异不在 CSP 处理路径 |
