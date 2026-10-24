# 技能启用开关：拨杆形态 + 关掉即不可用

> 2026-10-24 · 现行实现口径见 `SPEC.md`「技能系统」条。
> 本文只写「为什么这么切」与「哪些坑踩过」——代码结构与判据以源码为准。

## 一、为什么要改

技能系统的 `enabled` 字段一直存在（frontmatter `enabled` 键），但**没有入口**：
用户想关掉一个技能，只能自己去技能目录里手改 `SKILL.md` 的 frontmatter。
这条路的代价有三：

1. **不知道在哪改**——设置页「打开技能目录」只能看到目录层级，得自己找
   `SKILL.md`、自己动 YAML；
2. **不知道改没改生效**——改完要重启才知道，而重启前的清单里那个技能还在；
3. **不敢关**——清单里混着根本用不上的技能（演示模板、临时调研），每个都在
   吃系统提示词的 token，用户看不出哪些该关。

Claude Code 为这件事专门做了 `/skill-doctor`：列出「从没被调用过」的技能并
告诉用户该去关哪个（`code.claude.com/docs/en/skills`）。**大厂把「关技能」
当成正经的上下文治理动作**——技能清单本身是每轮都在付的 token。

所以这次的诉求是两条：**设置页每条技能最右边一个拨杆**（解释走悬停，不占行内
宽度），以及**拨杆关掉后这个技能模型就调不到**。

## 二、业界怎么做（调研）

### 2.1 Anthropic Claude Code

现行 skills 文档（`https://code.claude.com/docs/en/skills`）给出**两条路**：

**路 A：状态写在技能自己的 frontmatter 里**

> **Hide individual skills** by adding `disable-model-invocation: true` to their
> frontmatter. This removes the skill from Claude's context entirely.

> With `user-invocable: false`, you can't invoke the skill, but Claude still can.
> To keep Claude from invoking it through the Skill tool, set
> `disable-model-invocation: true`.

即「不让模型调」= **frontmatter 一个键**；关掉后技能**从模型的上下文里彻底消失**
（不只是调不动，是看不见）。

**路 B：状态写在设置覆盖表里**（不动技能文件）

`skillOverrides` 设置项，四态：

| 值 | Listed to Claude | In `/` menu |
| :- | :- | :- |
| `"on"` | Name and description | Yes |
| `"name-only"` | Name only | Yes |
| `"user-invocable-only"` | Hidden | Yes |
| `"off"` | Hidden | Hidden |

> The `/skills` menu writes it for you: highlight a skill and press `Space` to
> cycle states, then `Esc` to save to `.claude/settings.local.json`.

**路 B 的适用场景文档写得很明确**：

> The `skillOverrides` setting controls skill visibility from your settings
> instead of the skill's own frontmatter. Use it for skills whose SKILL.md you
> don't want to edit, such as ones checked into a shared project repo.

### 2.2 VS Code（1.109，2026-01）

`https://code.visualstudio.com/updates/v1_109` 的「Use skills as slash commands」一节：

> Agent Skills are now available as slash commands in chat, alongside prompt
> files. […] Use the `user-invocable` and `disable-model-invocation` frontmatter
> properties in your skill files to control how each skill is accessed: Set
> `user-invocable: false` to hide a skill from the menu while still letting the
> model load it automatically. Set `disable-model-invocation: true` to show the
> skill in the menu but prevent the model from loading it on its own.

即 VS Code 与 Anthropic **同源**（同一套 Agent Skills 规范、同一对 frontmatter
属性名），开关状态同样落在**技能文件自身**。

### 2.3 归纳

| 维度 | 观察 |
|---|---|
| 状态放哪 | 两家都以 **frontmatter 属性**为主（`disable-model-invocation` / `user-invocable`）；Claude Code 另设 `skillOverrides` 表作为「不改技能文件」的旁路，专治「技能在共享仓库里、不该被本地改」 |
| 关掉的语义 | 不是「调不动」，是**从模型上下文里消失**（Claude 原文：removes the skill from Claude's context entirely） |
| UI 形态 | 列表项旁的开关 + 状态循环（Claude 的 `/skills` 菜单：选中按 `Space` 循环态） |
| 附加价值 | 关技能 = 省上下文；官方专门做 `/skill-doctor` 提示哪些该关 |

## 三、本次的口径

### 3.1 单一事实源 = 技能自身 frontmatter 的 `enabled` 键

沿用**路 A**：写进技能自己的 `SKILL.md` frontmatter。

**为什么不另开状态表**（即不做路 B）：

- 本项目的技能是**应用本地副本**——用户导入的在 `app_data_dir/skills/`，内置的
  物化在 `app_data_dir/builtin_skills/`，都不是「共享仓库里、不能改的文件」。
  路 B 存在的理由（别改别人仓库里的文件）在这里不成立；
- 路 B 会造成**双事实源**：frontmatter 里一个 `enabled`、设置表里又一个覆盖值，
  用户手改 SKILL.md 时不知道该改哪一份，两份漂移后表现为「我明明关掉了它还在」；
- 路 B 的额外形态（`name-only` / `user-invocable-only`）在本项目没有对应需求——
  本项目的技能只有「模型能不能用」这一个维度（`user-invocable` 那种「用户能调、
  模型不能调」的分身没有场景）。

**已有三道闸本来就在读 `meta.enabled`**，这次只补「写路径」，闸本身一行没改：

| 闸 | 位置 | 效果 |
|---|---|---|
| 清单过滤 | `manage::build_skill_block` / `build_skill_block_for` | 关掉的技能**不再被广告给模型**（系统提示词里不出现） |
| 意图路由 | `manage::intent_rules_from_dirs` | 关掉的技能**不产出路由条目**（用户消息不会被路由进去） |
| 启动预审 | `runtime::preflight` | 关掉的技能 `use_skill` **启动即拒**（「技能「x」已被禁用」） |

三道闸合起来 = Claude 原文的 "removes the skill from Claude's context entirely"：
**看不见、调不动、路由不劫持**。

### 3.2 写哪一份：生效副本

`skill_search_paths` 的优先级是 **用户 skills > 内置物化副本 > dev mock**
（debug build）。同名技能可能同时存在好几份，写错副本就是「关的是 A、生效的是 B」
——用户点了开关没反应，或关不掉真正生效的那份。

所以 `skills_set_enabled` 用 `effective_skill_md()` 按**同一个优先级**取第一份
存在的 `<dir>/<name>/SKILL.md`，与读取侧 `state::load_skill_meta` 的 fallback
同口径。

### 3.3 内置技能：应用升级不得把用户关掉的技能重新打开

内置技能在启动时物化（`builtin::materialize_one`）：磁盘版本 ≠ 资产版本 → **覆盖**。
若不管，用户在设置页关掉的技能会在下一次应用升级后**自己又开了**。

口径：**磁盘副本里显式声明了 `enabled: false` = 用户的手动决定，跨版本保留**；
没声明（`None`）= 用户没表态，跟随资产默认值。

只保留 `false` 不保留 `true`：资产默认恒为启用（`builtin_assets_parse_clean`
锁死），保留 `true` 与跟随资产等价，多写一份状态只会多一个漂移源。

> 注：`pptx-design` 已于 2026-10-24 退役（用户自装技能，应用不再随包分发），
> 物化机制保留但**资产清单当前为空**——这条口径等下一个内置技能进来时生效。
> 退役本身带一条清理：物化目录里不在清单中的副本全删（否则残留副本 = 列表里
> 删不掉的僵尸技能，见 `SPEC.md`「内置技能已退役」条）。

### 3.4 UI 口径

- 每行**最右边**一个拨杆（`Toggle` 组件，`size="sm"`，`role="switch"`），
  删除图标在拨杆左侧；**按钮后不跟文字解释**，解释走悬停 tooltip；
- 关掉的技能**整行置灰**（名称降到 `--t5`）——对应 Claude 的禁用态灰显惯例；
- 后端没下发 `enabled` 字段时**视为启用**（缺省不误关）。

## 四、踩过的坑

1. **BOM 用 `strip_prefix` 取是错的**——`strip_prefix` 返回**剩余部分**而不是
   匹配的前缀。BOM 必须单独判（`starts_with('\u{feff}')`）再手工接回，否则
   BOM 文件的 frontmatter 坐标整体错位一格。
2. **重复 `enabled` 键要改最后一个**——解析器逐行赋值、同名键**后者覆盖前者**。
   改第一个 = 改了那个不生效的键（开关点了没反应）。源文件里本来就存在过两个
   `enabled` 的情况。
3. **多行块的续行不是顶层键**——`intents:` / `params:` / `allowed-tools:` 的空值
   形式会吞掉后续 `- ` 开头的行；把 `allowed-tools` 里的工具名当成 `enabled` 键
   改掉 = **改坏了技能**。
4. **行尾注释会让值失真**——解析器**不剥行尾注释**，所以 `enabled: false  # 关闭原因`
   会被读成 `true`。整行替换（而不是只改值）才能把值定死。
5. **写完必须能被解析器读回**——`set_frontmatter_enabled` 收尾自检：改写后
   `parse_meta` 读回来的值必须等于目标值，读不回就返回 `None`、宁可不写。
   写一份「开关点了没反应」的技能比报错更糟。
6. **意图路由是进程级缓存**——`intent_rules_from_dirs` 的结果进进程级路由表，
   改完必须 `rebuild_intent_routes(app)`，否则关掉的技能仍会产出路由条目、
   继续劫持用户消息。
7. **原子写**——同目录 tmp + rename。中途断电留半份 `SKILL.md` = 整个技能失效。

## 五、实机验收判据

1. 设置页「机器人技能」区，每条技能最右有一个拨杆；**拨杆后没有文字**，
   鼠标悬停出解释（已启用 / 已停用两种文案不同）；
2. 关掉某个技能 → 重启应用 → 该技能仍是关的（拨杆态不变）；
3. 关掉的技能在聊天里问它相关的事：模型**看不到**这个技能（清单里不出现），
   也不会被意图路由劫持进它；
4. 手动 `use_skill` 调一个已关掉的技能 → 明确拒绝（提示「已被禁用」）；
5. 重新打开 → 技能回到清单，路由恢复；
6. 关掉某个内置技能 → 升级内置技能版本（改资产 `version`）→ 重新物化
   → 该技能**仍是关的**（当前资产清单为空，此条等下一个内置技能进来时验）。

## 六、不做的

- **`name-only` / `user-invocable-only` 中间态**：本项目只有一个「模型能不能用」
  的维度，用户侧手动调用技能不存在，不引入；
- **设置覆盖表（路 B）**：没有「技能在共享仓库里、不能改」的场景，且会造成
  双事实源（见 §3.1）；
- **按技能粒度的策略配置**：拨杆本身就是那个粒度，再加一层是重复抽象。
