# Batch Spec: U18-MEMEVAL

## 目的

记忆系统第三期第一斧：给记忆系统装「尺子」——检索质量量化基线 + 抽取产物
人工标注采样。只建测量基础设施，不改任何生产行为（src-tauri/src 零改动）。

- **黄金查询集** `fixtures/memory_golden.json`：42 条拟真中文记忆种子
  （画像/偏好 8、事实 12、教训 6、改口对 5×2、干扰 6，全为 ZCode 生成、
  非真实用户数据，**待老板抽审**）+ 30 条查询（10 画像偏好 / 10 稳定事实 /
  5 经验教训 / 5 混合改口；改口查询的 stale 条目作干扰项考排序）。
  播种时 `dedup_merge=1.0` 关闭语义合并，保证一条一行、种子 id 稳定可引用。
- **评估器** `src-tauri/tests/memory_eval.rs`（独立测试二进制，两个
  `#[ignore]` 手动测试，`--ignored --nocapture` 跑）：
  - `eval_recall_report`：真实 ONNX 嵌入播种内存库 → 逐条 query 走生产
    同构 `rank::hybrid_search_with`（`RankParams::of(&MemoryTuning::default())`）
    → 打印 recall@1 / recall@5 分项与总榜 + 未命中明细；断言总体
    recall@5 > 0 仅防评估器自身坏掉（嵌入失效/检索全零），不设质量门槛。
  - `eval_extract_sample`：从真实记忆库（`WMESSAGE_EVAL_DB` 指定或 macOS
    默认用户库）**只读**打开，随机抽 30 条 model_inferred 抽取产物
    （profile/preference/fact，系统 summary/其他 source 不采）导出
    `fixtures/memory_extract_sample.jsonl`（content/kind/importance/
    created_at + human_label 留空）供人工标注噪音率。导出文件含隐私内容，
    已进 .gitignore 不入库。
- **W3 调优判定**：基线 recall@5 = 0.90 ≥ 0.6，且无噪音标注数据 →
  本批不动权重/阈值/提示词（无数据不动）。

## 基线数字（2026-10-03，本机 M 系列真实 bge-small-zh-v1.5 推理）

| 分项 | recall@1 | recall@5 |
|---|---|---|
| 画像偏好（10） | 0.60（6/10） | 0.90（9/10） |
| 稳定事实（10） | 0.60（6/10） | 0.80（8/10） |
| 经验教训（5） | 1.00（5/5） | 1.00（5/5） |
| 混合改口（5） | 0.60（3/5） | 1.00（5/5） |
| **总榜（30）** | **0.6667（20/30）** | **0.9000（27/30）** |

- 两次运行逐项一致（确定性：嵌入与打分均纯计算，固定时间基准播种）。
- 未命中 3 条（top5 无期望条目）：q-p04「什么时候找用户说话合适」、
  q-f02「用户的服务器在哪个机房」、q-f03「用户什么时候打球」——均为
  查询词与内容字面零重合、语义贴近度不足以挤进 top5，属记录不修。
- **U19 验收锚点**：recall@5 基线 = 0.90；U19 合入后复跑不得低于此值。
- `eval_extract_sample`：真实用户库当前无 model_inferred 抽取产物
  （autoExtract 从未开启），空库路径实测正常退出；采样逻辑另以临时库
  实测（2 条导出，kind/source 过滤正确，human_label 留空）。

## ocr 复审处置记录（3 条 1H/1M/1L，json 于 docs/OCR-CODE-REVIEW-2026-10-03-u18.json）

- **本批文件 0 findings**：memory_eval.rs / memory_golden.json / spec / .gitignore
  均无问题被提出。
- **不适用 3**：全部落在 `model-meta-service/main.py`（hash() 随机化导致
  颜色漂移 / fallback 字典键覆盖 / 计数器语义）——untracked 未入库、U12 已
  拍板废弃的独立 Python 服务目录（U15/U16 同口径），不随本批处置。

## 红线核对

- 生产行为零变化：本批零触碰 src-tauri/src，仅新增测试二进制 + fixture。
- 评估器只读真实库（SQLITE_OPEN_READ_ONLY），绝不写用户数据。
- 黄金集为拟真种子（ZCode 生成，非真实用户数据），spec 注明待老板抽审。
- 默认档不变：autoExtract 默认 off、memoryTuning 缺字段全默认均未触碰。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U18-MEMEVAL",
  "family": "memory-phase3",
  "expected_files": [
    ".gitignore",
    "DEVLOG.md",
    "docs/batches/U18-MEMEVAL.spec.md",
    "fixtures/memory_golden.json",
    "src-tauri/tests/memory_eval.rs"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 5,
  "max_new_files_lines": 500,
  "findings": [
    { "file": "src-tauri/tests/memory_eval.rs", "note": "真实库只读打开（READ_ONLY flag）；黄金集播种关语义合并保 id 稳定；recall@5>0 仅防评估器自身坏掉不设质量门槛；行 id→种子 id 映射后判定命中" },
    { "file": "fixtures/memory_golden.json", "note": "42 拟真种子 + 30 查询（10 画像/10 事实/5 教训/5 改口），改口查询带 stale 干扰项；条目为生成文本待老板抽审" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
cargo test --test memory_eval eval_recall_report -- --ignored --nocapture   # 基线报告，两次一致
cargo test --test memory_eval eval_extract_sample -- --ignored --nocapture  # 采样导出（空库路径 ok）
bash scripts/test-fast.sh                                                   # 过
```
