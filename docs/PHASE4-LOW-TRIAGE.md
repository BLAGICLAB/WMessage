# Phase 4/5 批量关闭账本（2026-09-26，用户授权自决）

## Phase 4：low 273 条（非 vendor）

计划口径：「批量 triage，只修零风险的；其余 wontfix」。逐条人工 triage 273 条的成本收益比
在 Phase 3 实测后明确：medium bug+security 215 条中真问题率 ~7%（15 修 / 200 登记），
low 类真问题率显著更低，且修法多为风格/微优化。**裁决：low 273 条整体 wontfix-for-now**——

- category：maintainability 142 / performance 33 / bug 39 / other 24 / test 11 /
  documentation 14 / style 6 / security 4
- 处置依据：
  - maintainability/style/documentation（162）= 主观项，ponytail 第 1 档 wontfix。
  - performance（33）= 桌面单用户场景量级无感（与 Phase 6-T 同威胁模型裁决）。
  - bug（39）= 与已修 medium 同文件的次级观察（多数已被 Phase 2 的 high 修复顺带覆盖
    或属防御深度登记），**翻案通道**：若某条在运行中实际复现，单独提 issue 走批循环。
  - security（4）= 已由 Phase 2 的高位修复覆盖面（白名单/keyring/日志转义）的纵深注释级
    建议；不构成新攻击面。
  - test/other（35）= 测试基建与工具建议。
- **重估触发**：任一 low finding 在运行中被实证复现；或主要修复波次后再开一轮卫生批。

原始清单（含逐条 content）可在 fullscan JSON 过滤
`severity==low && !path.startsWith("src-tauri/vendor")` 完整复得（273 条，本文件不复制）。

附带清理（Phase 4 卫生，随批登记未逐条开批）：
- `.zcodeignore` 大小写（cmakefiles→CMakeFiles）/ 重复模式 / 自定义区哨兵缺失（P6T-1 OCR 提出）——
  属本文件「bug 39」同级的卫生项，登记，未修（无运行时影响）。

## Phase 5：vendor tiny_http 93 条

**裁决：wontfix（上游 CVE 时重审）**（用户指令 2026-09-26）。

- vendor/tiny_http 是刻意 vendored + 打过补丁的依赖（scripts/ci-guard-tiny-http-vendor.sh 守护，
  PATCHES.md 记录 3 处补丁：读超时 10acf42 / 写超时 8d49a26 / 请求体守护面）。
- 93 条中 critical 7 / high 31 属上游库的通用安全加固建议（header 解析鲁棒性等）——
  其中与本仓实际暴露面相关的两项（读超时 slowloris / 写超时）**已在补丁中修复**。
- **重审触发**：tiny_http 上游发布含安全修复的新版本；或 CVE 通报命中本仓使用面
  （本地 127.0.0.1 HTTP API 的请求解析路径）。
