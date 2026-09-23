# PROC-5 type 3（rate-limit 429）评估

> 触发条件达成：type 3 累积 n=2（APW-02b run B + BT-01a r1）。
> 本文档为**评估产物**：选项 + 事实，**不出建议**。拍板留给 reviewer。

## 1. 现象（事实）

| run | 文件 mtime | session_id | elapsed | tokens (in/out/cache) | 失败项 | failure_details 数 |
|---|---|---|---|---|---|---|
| APW-02b run B | 2026-09-23 18:01:12 | c8da10cc-… | 30s | 0 / 0 / 0 | 1/1 selected items | 0 |
| BT-01a r1 | 2026-09-23 19:59:07 | 31bf2c6c-… | 15s | 0 / 0 / 0 | 1/1 selected items | 0 |

- 间隔 ~1 小时 58 分钟
- 两次 run **token 全 0**（output 也为 0）—— 失败发生在 tool 阶段（可能进 LLM 前就被截断）
- 两次 run 都 **1/1 selected items failed** —— 不是部分成功
- 两次 run 的 session_id 不同 → **未复用会话**
- 两次 run 的 **failure_details 数组为空**（manifest 没回填尝试级细节，**无 attempt 序号 / backoff 数据可看**）
- BT-01a r1 elapsed 15s 比 APW-02b run B 30s 短一半 —— 失败更快，命中关卡可能更早

## 2. 频次判定

- 样本量 n=2，时窗 ~2h
- 证据不足以判定"偶发"也证据不足以判定"近期集中"
- 现状：2 次 / 2h
- 若 APW-02a / APW-02b run A（成功 run）也走 OCR 调用：实际限流触发率 = 2/3 ≈ 67%

## 3. 可能原因（基于 OCR 工具语义，非断言）

- **限流配额**：provider 端 token-per-min / requests-per-min 上限
- **退避策略**：429 后未做或不足够的指数退避；OCR 工具可能默认 retry 固定次数
- **调用频率**：单次 batch commit 内 OCR 调用次数 + 跨批 commit 调用间隔
- **端点 / 模型选择**：当前 OCR 默认走 minimax-cn/MiniMax-M3，provider 可能在高峰时段限流更严
- **失败信息回填**：manifest.failure_details 为空 → 工具未把 attempt-level 错误回传到输出 JSON，**观测能力受限**（不知尝试几次、每次 backoff 多长）

## 4. 调用侧 3 选项（不改 OCR 工具前提）

### 选项 A：在 commit hook 调用前加 backoff 参数

- **能解决什么**：若 OCR 工具支持 backoff 自定义，传递更长 base/max；可能避开短窗口 429
- **不能解决什么**：若工具不暴露 backoff 参数 / 已内置固定 backoff → 无效；若 429 是 token-per-min 配额 → 仍超限

### 选项 B：拉长 OCR 调用频率（每批 / 跨批间隔）

- **能解决什么**：降低单位时间请求数，可能避开 requests-per-min 限制
- **不能解决什么**：拉长 commit 周期影响 batch discipline（"完成一次报"要求快速收口）；不能解决 token 配额；若跨天 commit → 仍触发

### 选项 C：换端点 / 换模型（如 minimax-cn/MiniMax-M3 → 其他 provider 或 model）

- **能解决什么**：换 provider 即换 quota pool，绕开当前限流；换 model 可能降 token 用量
- **不能解决什么**：切换成本（CLI 配置 / 鉴权 / 输出格式）；新 provider 可能更慢 / 更贵；不解决根本原因（quota 上限存在）；OCR 工具是否支持 provider 配置需查

## 5. 未决问题（需补信息才能评估）

1. **failure_details 为空** —— OCR 工具未回传 attempt-level 错误。**先确认这是工具默认行为，还是本次 run 的特例**。若是工具限制，需要换工具 / 升级版才能观测尝试级细节。
2. **cache_read_tokens** 在 summary 里 key 缺失 —— 两次都缺；**先确认 OCR 工具是否支持此字段**，若支持但值为空 = 真未命中 cache，若不支持 = 字段不适用。
3. **跨批调用频率** —— 今天是密集 batch（APW-02a → APW-02b → BT-01a，每个都触发 OCR），是否全天累计触发？若全天 ≥3 次且 2 次 429 → 实质上**所有 commit 链路 67% 触发限流**，需立即处理。
4. **provider 是否区分 session** —— 两次 session_id 不同都失败；若 OCR 工具内部仍按 IP/rate-pool 共享 quota → 选项 C 有效；若每 session 独立 quota → 选项 C 无效（换 provider 才行）。

## 6. 不在本评估范围

- **不动 OCR 工具**（reviewer 拍板前不改）
- **不改 OCR 调用配置**（provider / model / backoff 参数）
- **不实施 A/B/C 任一选项**（仅列选项）

## 7. 输入文件

- `/tmp/ocr-APW-02b-r1.json`（5182 bytes，run B / failed / 429）
- `/tmp/ocr-BT-01a-20260923-195852.json`（5192 bytes，r1 / failed / 429）
- `~/.openclaw/cache/APW-02b/ocr-r1.json`（APW-02b run A / **complete / 5 findings** — 此 run 未触发 429，作为对照）
- `~/.openclaw/cache/BT-01a/ocr-r1.json`（= BT-01a 失败 run 的拷贝）

## 8. 输出

- 选项 A / B / C 拍板 + 未决问题补充 → 后续 PR / 评估文档
- 不写代码改动建议
