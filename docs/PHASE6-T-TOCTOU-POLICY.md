# Phase 6-T：TOCTOU/权限统一策略（2026-09-26 自决版）

> 用户授权主控自决（2026-09-26）。威胁模型锚定：**本地单用户桌面应用**——代码以 OS 用户身份运行，
> 用户即最高权限持有者；「本地竞态攻击者」（另一进程协同 swap 文件）不在核心威胁模型内，
> 仅作为纵深防御的残余风险登记。fsync/fd/O_NOFOLLOW 类内核原语在 Tauri opener/IPC 面不可达。
> 统一裁决原则：**只读工具的残余 TOCTOU = 接受并登记；写路径已有 fail-closed 的维持；可用性 > 不可达边界的纵深。**

## 裁决表

| ID | 项 | 裁决 | 理由 | 代码 |
|---|---|---|---|---|
| C1b-1 | grep/list walk 无 inode re-check | **接受** | 只读 + 入口 canonical 白名单检查；walk 期被换最坏 = 读到另一个用户可读文件，无权限边界穿越 | 无 |
| C1b-2 | resolve_with_perm fd 不复用 | **接受** | 同 C1b-1 攻击模型；fd 贯穿 API 改造复杂度不值（read-only） | 无 |
| C1b-3 | symlink target swap | **接受** | 同上；target swap 仅影响读到的内容，不越白名单语义 | 无 |
| C1b-4 | Windows 端不捕获 inode | **维持 fail-open** | 主平台 macOS；Windows 可用性优先。已知限制登记：Windows read_text_file 无 TOCTOU 保护 | 无 |
| C1b-5 | capture_pre_ino None 语义 | **维持 fail-open** | metadata 失败即拒 = read_text_file 在边缘环境整体不可用；check 本就是纵深而非唯一闸 | 无 |
| C1b-6 | spawn_blocking_map 丢 ErrorKind | **wontfix-with-rationale** | 无调用方按 kind 分流；String 已是用户面错误形态；改契约 ripple 全部调用方，零现役收益 | 无 |
| C1b-7 | walk / 白名单读失败静默 | **修（本批）** | read_dir / entry / JoinError 三处补 eprintln 可见化，语义不变 | bot_fs.rs |
| C2a-1 | reveal_item_in_dir 无 scope | **维持 + 触发器** | 权限模型不动（红线）；前端现无调用方；重估触发 = 任何前端引用该命令 | 无 |
| C2a-2 | SkillsPanel openPath 迁 Rust | **defer 维持（2026-09-26 拍板 B）** | 触发条件：便携模式用户 openPath/app_data_dir 真实报障，或需摘 $APPDATA scope 时随批迁 Rust command；无报障不做防御性迁移 | 无 |
| C2a-3 | app_data_dir 失败分支无测试 | **defer** | 3 行 fallback 的 mock 成本 > 价值 | 无 |
| C2c-v1 | recheck→副作用内核级窗口 | **接受** | opener API 无 fd/O_NOFOLLOW 暴露面；recheck_canonical 已缩到最小；剩余 = 内核原语限制，登记已知边界 | 无 |

## 残余风险登记（接受项汇总）

- 读类工具（read/grep/list）在白名单检查与实际 IO 之间存在理论竞态窗口（C1b-1/2/3、C2c-v1）；
  实际影响 = 本用户可读文件内容进入模型上下文，无提权、无越界写。
- Windows 端 read_text_file 无 TOCTOU 保护（C1b-4/5）。
- 重估触发器：应用引入多用户/服务模式；Tauri opener 提供 fd 能力；出现真实越界事件。
