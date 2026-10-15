# APP-DEV-CONVENTIONS：程序模块开发规范

> 「程序」（apps）= 大模型**开发时**写好的确定性小程序（脚本 + 输入声明），用户运行时
> 点一下跑，不连大模型。本文是 AI 上架工具时的代码规范——M2 起由 LLM 调用
> `apps_create` 上架时按此执行，M1 手工造工具时同样适用。
>
> 设计上下文：[APPS-DESIGN-2026-10-15.md](./APPS-DESIGN-2026-10-15.md)。

## 1. 目录结构（每个 app 一个子目录）

```
{data_dir}/apps/<id>/
├── app.py             # M1 恒为 app.py（script_path 字段）
├── manifest.json      # 可选：完整 manifest 的 JSON 副本（与库表镜像）
├── data/              # 常驻数据（人员变动才更新；姓名-身份证对照表等）
│   └── ...
├── input/             # 每次运行前放入（本月工资单等）
│   └── ...
└── output/            # 运行产物（脚本写入；按 run_id 子目录防覆盖）
    └── <run_id>/
        ├── ...
        └── ##RESULT## 哨兵仅写 stdout/stderr
```

`data_dir` 在 macOS 是 `~/Library/Application Support/wmessage/`、Windows 绿色版跟随
exe 根（便携模式）。app 目录由 `apps_create` 命令**原子化建好**——AI 写代码时
不要手工 `mkdir`，命令会建。

## 2. manifest 字段（apps 表的契约）

字段名（camelCase）即后端 `db::apps::App` 字段名（serde camelCase 直出）：

| 字段 | 类型 | 约束 | 说明 |
|---|---|---|---|
| `id` | string | **必填**，路径分量，不能含 `/` `\` `..` | 同上：程序目录名 |
| `name` | string | ≤ 200 字符 | 列表卡片标题 |
| `description` | string | ≤ 500 字符 | 列表卡片副标题 |
| `icon` | string | ≤ 4 字符（emoji） | 列表卡片图标 |
| `inputSchema` | JSON 字符串 | **必填**合法 JSON | 见 §3 |
| `scriptPath` | string | M1 恒 `app.py` | 相对 app 目录的脚本路径 |
| `manual` | string | ≤ 8000 字符 | 程序页的「管理/说明」用 |
| `status` | enum | `draft` / `enabled` / `archived` | 上架时建议直接 `enabled` |

`enabled_at` / `created_at` / `updated_at` 由命令自动写，不要在 manifest 里给。

## 3. inputSchema 两种模式

**文件夹模式**（最简，推荐先从这开始）：
```json
{"kind":"folder"}
```
约定 `data/ input/ output/` 三个子目录，脚本自己读。

**表单模式**：
```json
{
  "kind": "form",
  "fields": [
    {
      "key": "month",
      "label": "月份",
      "type": "date",         // text | number | date
      "required": true,
      "placeholder": "2026-10"
    },
    {
      "key": "threshold",
      "label": "阈值",
      "type": "number"
    }
  ]
}
```
用户点运行后按 `fields` 顺序弹窗收集 → 拼成 JSON 写 `params.json` → 脚本读。

## 4. 脚本契约（运行器校验）

脚本写在 `app.py`，python 3，**零依赖**为佳。运行时由 `py::run_python` 子进程启动，
**`sys.argv[1]` = app 目录绝对路径**（runner 传入——子进程 cwd 在临时 run 目录，
脚本解析 `data/ input/ output/` 一律基于 argv[1]，不要依赖 cwd）、`PATH` 继承、
有 60s 超时（可调）。以下约定保证运行器能稳定采集结果：

### 4.1 开头校验段（强制）

```python
# 头三件事：参数/文件存在/列名齐全
import sys, os, json
params_path = os.environ.get("APPS_PARAMS", "")
if params_path and os.path.exists(params_path):
    with open(params_path, "r", encoding="utf-8") as f:
        params = json.load(f)
else:
    params = {}
# 校验 input/ 列名/存在性
required = {"姓名列", "身份证列"}
if not os.path.exists("input/本月.xlsx"):
    print("❌ input/本月.xlsx 不存在", file=sys.stderr)
    sys.exit(2)
```

**为什么强制**：输入格式漂移是程序最常见的死法，对得上就过、对不上**响亮报错**
而不是静默跑出空结果。脚本里用 stderr 写「❌ + 中文 + 简短原因」，运行器会捕
获并写进 `app_runs.summary`。

### 4.2 收尾 `##RESULT##` 哨兵（强制）

脚本**最后一行**必须是 `##RESULT## <JSON>`，运行器会按**倒序找最后一条**匹配。

```python
# 推荐结构
result = {
    "summary": "匹配 42 条，未匹配 3 条",   # 必填；通知用
    "matched": 42,                            # 可选
    "unmatched_count": 3,                    # 可选
    "unmatched_path": "output/<run_id>/未匹配.csv",  # 可选
    # ... 任意字段
}
print("##RESULT## " + json.dumps(result, ensure_ascii=False))
```

**规则**：
- 只取**最后一条**哨兵（前面打印的 `##RESULT## ...` 行会被忽略）
- 哨兵必须是合法 JSON，否则运行器取不到 summary、summary 字段会兜底为整段文本
- 哨兵前**必须有换行**——直接 print 在别的内容后面会粘到同行导致解析失败
- 想给用户看的长信息写 `summary`（≤300 字符会被通知面板截取），详细产物写产物文件

### 4.3 不要做

- ❌ 联网（默认信任单用户本机；联网要走专门授权）
- ❌ `input/ output/` 之外写文件（破坏目录契约）
- ❌ 长时间死循环或阻塞（60s 超时会被强杀）
- ❌ 在 `app.py` 里硬编码敏感数据（身份证号、工资数等）——放在 `data/` 配文件里
- ❌ `print` 大量调试信息到 stdout（>64KB 会被截断；调试写 stderr）

### 4.4 失败语义

| 退出码 | 语义 | 运行器处理 |
|---|---|---|
| `0` | 成功，stdout 找哨兵 | status=ok |
| `2` | 输入校验失败（缺文件/列名） | status=failed，stderr 写 summary |
| 其他非 0 | 运行时错误 | status=failed，stderr 写 summary |
| 超时 60s | 强杀 | status=failed，「超过 60 秒未完成」 |

## 5. M1 与 M2 边界

**M1（当前）**：
- 上架**手工**完成——你（或我）写 `app.py`、写 manifest 草稿、在程序页填表单上架
- Tauri 命令已就位：`apps_list_active` / `apps_list_archived` / `apps_get` /
  `apps_run` / `apps_create` / `apps_update` / `apps_archive` / `apps_unarchive` /
  `apps_runs_list` / `apps_versions_list` / `apps_version_restore` /
  `apps_preflight` / `apps_open_output`
- 程序页「删除」= 归档（可逆）：`apps_archive` 只改生命周期状态，脚本与数据不动；
  「已归档」视图走 `apps_list_archived`，恢复走 `apps_unarchive` → 草稿态。
  已归档程序既不能运行（`apps_run` 拒）也不开放修改（`apps_update` 与 bot 工具
  同口径拒绝），要改先恢复
- 约束验证：脚本可读+非空（且必须是 app 目录下的单个文件名）、id 路径分隔符检查、
  status 枚举

**M2（2026-10-15 已落地）**：
- 三个 bot 工具进 TOOLS_TABLE：`apps_list`（列非归档程序防重名）/ `apps_create`
  （落库**草稿** + 文件集落盘）/ `apps_update_files`（整文件集修改，定名取「整文件
  集提交」语义；本节草案曾用名 `apps_update` 作废）
- AI 改写时**整文件集提交**（不只单字段），避免与 LLM 增量编辑的复杂合并：
  `script` 传了就整体重写 `app.py`（不收 diff/片段），其余字段不传 = 保持不变
- 脚本类型校验：`inputSchema.form.fields` 类型必须是 `text` / `number` / `date`
  三选一；§2 的 manifest 长度契约在 Tauri `apps_create` 与两个 bot 工具三口同校验
- 试跑闭环：草稿在程序页可见（草稿徽章）且可运行，`apps_run` 只拒绝 archived；
  上架 = AI 在用户明确满意后调 `apps_update_files` 传 `status="enabled"`
- 附加文件边界：bot 工具只允许写 `data/` 之下（input/ 归用户、output/ 归运行）
- 版本快照：程序本体（manifest 五项 + `app.py` 全文）在创建/更新/回滚时自动存
  `app_versions` 表，每程序滚动保留 100 版；程序页「历史 → 版本」页签可回滚——
  回滚 = 旧内容整体写回（生命周期状态保留），本身记为新版本、可再回滚撤销。
  拍板口径：快照表而非 git（Windows 绿色版不带 git、`data/` 敏感数据不进历史）；
  知识库级多文件语料将来走「独立数据目录 + 按需 git」，与本表正交
- 摘要截断：summary > 300 字符截断 + 标记截断，提示用户去 `output/<run_id>/` 看完整产物

## 6. 真机验收清单（M1 后）

- [ ] 上架一个空 `app.py`（只 print `##RESULT## {"summary":"hi"}`），程序页点运行能看到「任务完成」通知
- [ ] `id` 含 `/` 或 `..` 在上架时被拒（前端表单校验 + 后端命令 reject 双重）
- [ ] 删掉 app.py 后点运行，错误信息明确（`脚本文件不存在：…`）
- [ ] 历史面板能展示最近 10 条运行，包含耗时和 summary

## 7. 真机验收清单（M2 后）

- [ ] 挂件聊天里说「帮我做一个 X」，AI 用 read_text_file 读样例、ask_user 问规格、产出 `app.py` + manifest 并调 `apps_create` 上架
- [ ] 试跑：先用真实数据抽验失败一次 → AI 改脚本 → 试跑通过 → 上架
- [ ] 修改：聊天里说「字段名改了 X」，AI 调 `apps_update_files` 改整文件集

---

**M1 阶段 AI 不上架**——设计这条契约是为 M2 留好接口形状，避免到时候改 schema。
M1 期间上程序由人或脚本直接调 `apps_create`，参照本规范即可。
