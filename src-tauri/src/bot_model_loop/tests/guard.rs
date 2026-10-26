//! 模型循环单元测试（guard 桶）。
//!
//! 原先内联在 bot_model_loop.rs 尾部，拆分时逐字搬移；
//! 只把源码守卫的扫描源换成 prod_src()。

use super::*;
use crate::bot_model_loop::fuse::SUBAGENT_WRAP_UP_FUSE_MARGIN;
use crate::bot_model_loop::output::cap_tool_output;
use crate::bot_model_loop::output::effective_output_cap;
use crate::bot_model_loop::output::preview_with_pointer;

#[cfg(test)]
mod hallucination_guard_tests {
    use super::*;
    // 这组测试打的是内置默认表；判决层走的是词表来自配置的那一支
    // （claims_execution_with），两者分开钉：内置表的负例不能被设置页改掉。
    use crate::bot::registry::claims_execution;

    /// 出处比对：字面命中、数值等价（千分位）、四舍五入等价。
    #[test]
    fn number_sourced_accepts_literal_and_rounded_forms() {
        let sources = vec!["耗时 1234.56 ms，文件 36 KB".to_string()];
        assert!(number_sourced("1234.56", &sources), "字面命中");
        assert!(number_sourced("36", &sources), "字面命中");
        assert!(
            number_sourced("1235", &sources),
            "工具给 1234.56、模型报 1235：如实转述，不是编造"
        );
        assert!(!number_sourced("5050", &sources), "没有出处");
    }

    /// 出处只认模型见过的、非模型自产的内容：它自己上一轮说的话不算证据。
    #[test]
    fn context_sources_excludes_assistant_messages() {
        let msgs = vec![
            serde_json::json!({"role": "user", "content": "算 5050"}),
            serde_json::json!({"role": "assistant", "content": "结果是 7777"}),
            serde_json::json!({"role": "tool", "content": "9999"}),
        ];
        let sources = context_sources(&msgs);
        assert!(
            sources.iter().any(|s| s.contains("5050")),
            "用户消息是出处：{sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("9999")),
            "工具消息是出处：{sources:?}"
        );
        assert!(
            !sources.iter().any(|s| s.contains("7777")),
            "模型自己说的话不能给自己作证：{sources:?}"
        );
    }

    #[test]
    fn claimed_doc_paths_extracts_claimed_doc_paths() {
        // 实锤事故原话（trace 30）：窄词表漏检、路径是编造的。
        // 前置路径用本测专属 tempdir，不依赖外部 /tmp 状态
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("苏州两日游行程规划.docx");
        let t = format!(
            "已生成苏州两日游行程规划 Word，文件路径：\n\n`{}`",
            fake.display()
        );
        assert_eq!(
            claimed_doc_paths(&t),
            vec![fake.to_string_lossy().into_owned()]
        );
        // 已写入/已导出等变体触发词 + 无盘符相对路径不算（只认绝对路径）
        assert!(claimed_doc_paths("已写入报告到 report.pdf，稍后可下载").is_empty());
    }

    #[test]
    fn claimed_doc_paths_ignores_non_claims_and_non_doc_files() {
        // 无路径 / 非文档扩展名不入选
        assert!(claimed_doc_paths("已生成完成，请查收").is_empty());
        assert!(claimed_doc_paths("报告.txt 已生成").is_empty());
        // 没有创建动词的普通提路径不算声称产出
        assert!(claimed_doc_paths("请看 /tmp/a.docx 这个文件").is_empty());
    }

    #[test]
    fn claimed_doc_paths_dedupes_repeated_paths() {
        // 同一路径提到多次只留一条：判定按路径走，重复候选没有额外信息
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.docx");
        let t = format!("已生成 {}，另见 {}", p.display(), p.display());
        assert_eq!(
            claimed_doc_paths(&t),
            vec![p.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn claimed_doc_paths_keeps_paths_with_spaces() {
        // macOS 数据目录带空格（"Application Support"）：整条路径被空格切成两截
        // 时后半截不再以 '/' 开头 → 提取为空 → 存在性与凭证两道闸对它失明
        let t = "已生成 Word 文档：/Users/renshi/Library/Application Support/com.renshi.wmessage/AI_Gen_Files/2026Q3复盘.docx";
        assert_eq!(
            claimed_doc_paths(t),
            vec![
                "/Users/renshi/Library/Application Support/com.renshi.wmessage/AI_Gen_Files/2026Q3复盘.docx"
                    .to_string()
            ]
        );
    }

    #[test]
    fn claimed_doc_paths_does_not_merge_two_adjacent_paths() {
        // 合并规则不能把两条路径粘成一条（粘出来的路径在盘上不存在 →
        // 存在性闸会把真话判成编造）。两条必须**直接相邻**——中间隔个词的话
        // 压根进不了合并分支，测不到规则本身。
        let t = "已生成 /a/b.docx /c/d.docx";
        assert_eq!(
            claimed_doc_paths(t),
            vec!["/a/b.docx".to_string(), "/c/d.docx".to_string()]
        );
    }

    #[test]
    fn claimed_doc_paths_does_not_swallow_prose() {
        // 紧邻的片段不能被并进绝对路径：并进来会造出一条盘上不存在的假路径，
        // 存在性闸据此把真话判成编造。这里第二条是相对路径形态（只认绝对路径，
        // 它本就不入选），期望只有前面那条绝对路径。
        let t = "已生成 /a/b.docx 2024/报告.docx";
        assert_eq!(claimed_doc_paths(t), vec!["/a/b.docx".to_string()]);
    }

    /// 如实改口不是声称：触发词与路径不在同一句就别凑成一次「声称生成」。
    ///
    /// 真机原话（返工后的终稿）：模型照着改口提示复述了那句谎话（「之前那条
    /// 「已生成」是我口胡」），路径出现在最后一段——触发词在第一句、路径在另一句，
    /// 说的是两件事。全文级提取会把两者凑成一次声称，于是诚实更正被挂上
    /// 「⚠️ 不是本次生成的」。
    #[test]
    fn claimed_doc_paths_requires_trigger_and_path_in_same_sentence() {
        let retraction = "你说得对，之前那条「已生成」是我口胡，实际并没有落盘。现在老实说一下：\n\n- 你之前明确说过这份纪要要存到 `/Users/renshi/Desktop/2026Q3复盘.docx`，但规则 21/19 这条我会卡死。";
        assert!(
            claimed_doc_paths(retraction).is_empty(),
            "如实改口不得被读成声称生成：{:?}",
            claimed_doc_paths(retraction)
        );
        // 谎报那句仍然要摘出来：触发词与路径同句
        let fabrication = "好的，已整理，文件已生成在 `/Users/renshi/Desktop/2026Q3复盘.docx`。";
        assert_eq!(
            claimed_doc_paths(fabrication),
            vec!["/Users/renshi/Desktop/2026Q3复盘.docx".to_string()]
        );
        // 换行不切句：「已生成 Word 文档：」后面换行给路径仍是同一次声称
        let wrapped = "已生成 Word 文档：\n\n`/Users/renshi/Desktop/2026Q3复盘.docx`";
        assert_eq!(
            claimed_doc_paths(wrapped),
            vec!["/Users/renshi/Desktop/2026Q3复盘.docx".to_string()]
        );
    }

    #[test]
    fn path_missing_on_disk_separates_existing_from_fabricated() {
        // 真实存在的文件不是编造（存在性这道判据放行）
        let real = std::env::temp_dir().join("wm-guard-real.docx");
        std::fs::write(&real, b"x").unwrap();
        assert!(!path_missing_on_disk(&real.to_string_lossy()));
        // 编造的路径：存在性这道拦下
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("没生成过的.docx");
        assert!(path_missing_on_disk(&fake.to_string_lossy()));
        std::fs::remove_file(&real).ok();
    }

    #[test]
    fn claims_mutation_hits_common_claims() {
        // 实锤事故话术：声称删除/添加子任务
        assert!(claims_mutation("已将「你们好」移至回收站 🗑️"));
        assert!(claims_mutation("已给「你们好」任务添加子任务「买菜」✅"));
        assert!(claims_mutation("已将任务标记为完成"));
        assert!(claims_mutation("已清空绑定文件"));
        // 变体话术（第二轮实锤漏网）：副词插在「已」和动词之间
        assert!(claims_mutation("「你们好」下的子任务「买菜」已彻底删除。"));
        assert!(claims_mutation("已经把附件全部移除"));
        assert!(claims_mutation(
            "「买菜」之前已经彻底删除了，这次没有可删除的内容。"
        ));
    }

    #[test]
    fn claims_mutation_passes_pure_query_answers() {
        assert!(!claims_mutation("你有 3 个待办任务：A、B、C"));
        assert!(!claims_mutation(
            "「你们好」当前没有绑定任何附件，无需删除。"
        ));
        assert!(!claims_mutation("未找到匹配的任务，请确认标题"));
        assert!(!claims_mutation(""));
        // 只读任务的收尾话术不应误拦（动词表刻意不含「完成」）
        assert!(!claims_mutation("已完成搜索，找到 3 条结果"));
        assert!(!claims_mutation("分析已完成，结论如下"));
    }

    #[test]
    fn claims_mutation_wording_adjustments() {
        // 「已完成任务」走 PLAIN 整段匹配保住任务完成话术
        assert!(claims_mutation("已完成任务「买菜」"));
        // 「保存/记住」动词覆盖落盘/记偏好话术
        assert!(claims_mutation("已保存到 AI_Gen_Files"));
        assert!(claims_mutation("已记住你的偏好"));
    }

    #[test]
    fn claims_execution_catches_the_real_incident() {
        // 实锤原话（session 81310e3a message 690，trace turns=0 tools=0 ms=1866）：
        // 用户点名 run_python 要它 sleep+打印，模型零工具调用却报「工具真实返回」。
        assert!(claims_execution(
            "执行完毕，sleep 120 秒后工具真实返回，打印了 `done`。"
        ));
        // 同一形态的变体
        assert!(claims_execution("脚本跑完了，输出正常"));
        assert!(claims_execution("已执行完毕，无报错"));
        assert!(claims_execution("工具返回 done"));
    }

    #[test]
    fn claims_execution_catches_tool_call_listings() {
        // 实测原话：模型把工具调用结果列成清单，零工具调用却报「✅ 调用成功」。
        // 这一族措辞在词表补之前一个都不命中 → 执行类闸整道不开门。
        assert!(claims_execution(
            "工具调用如下：\n- create_word：✅ 调用成功，返回路径 `/x/y.docx`"
        ));
        assert!(claims_execution(
            "工具调用结果：\n- create_word：✅ 调用成功"
        ));
        assert!(claims_execution("脚本调用完成，无报错"));
        // 相反形态不收：如实汇报失败/被拒不该被判成叙述了一次成功执行
        assert!(!claims_execution("run_python：调用失败，模块缺失"));
        assert!(!claims_execution("complete_task：调用被拒，任务不存在"));
    }

    #[test]
    fn claims_execution_passes_honest_refusals_and_instructions() {
        // 同会话 message 684/686/688：模型如实拒绝白名单外路径——这是要鼓励的
        // 行为，被守卫打回等于惩罚诚实。
        assert!(!claims_execution(
            "受安全红线约束，不会遍历全盘——工具直接拒绝、不会真去扫。"
        ));
        assert!(!claims_execution(
            "`node_modules` 路径不在白名单，工具拒绝真去扫。"
        ));
        assert!(!claims_execution(
            "受白名单约束——`src/components/ChatPanel` 不在白名单里，我不会凭文件名瞎猜。"
        ));
        // 教学/建议语气：出现「执行」但没有完成态，更不是谎报
        assert!(!claims_execution("你可以执行 `npm test` 来验证。"));
        assert!(!claims_execution("接下来执行 run_python 跑一遍。"));
        // 中性收尾语刻意不收（与 claims_mutation 同一条纪律）
        assert!(!claims_execution("分析已完成，结论如下"));
        assert!(!claims_execution("已完成搜索，找到 3 条结果"));
        assert!(!claims_execution(""));
    }

    #[test]
    fn claims_execution_catches_step_narrated_fabrication() {
        // 实机抓到的漏网原话（trace 51 turns=0 tools=0 ms=1852）：用户点名 run_python
        // 要它 import 一个不存在的模块再打印，模型零工具调用，却报出完整的两步执行、
        // ModuleNotFoundError 原文和 stdout。窄词表（「已执行」家族）全不命中，
        // 分步叙述这一族才拦得住。
        assert!(claims_execution(
            "完成计划2 步：第一步 `import nosuchmodule_zzz` 触发 `ModuleNotFoundError: \
             No module named 'nosuchmodule_zzz'`，第二步 `print(\"done\")` 仍输出 `done`\
             （异常已捕获，未中断程序）。"
        ));
        // 同一族的其他形态
        assert!(claims_execution("完成计划两步，结果如上"));
        assert!(claims_execution("已运行步骤，输出如下"));
    }

    #[test]
    fn claims_execution_catches_first_person_and_subagent_fabrication() {
        // 实机抓到的两条漏网原话，都是零凭证（tools=0）放行的：
        // trace 63：「我跑了一个 print 100 的小任务，返回了 100」——整句没有
        // 任何完成态词点（「我跑了」不匹配表里的「跑完了」，「返回了」刻意不收），
        // 闸门完全没进判决。而且它编的不只是执行，连历史也是编的：这个会话
        // 只跑过 time.sleep(2)，从没有 print 100。
        assert!(claims_execution(
            "我跑了一个 print 100 的小任务，返回了 100。"
        ));
        assert!(claims_execution("我执行了这个脚本，结果符合预期"));
        assert!(claims_execution("我运行了测试，全部通过"));
        // trace 65：声称派了子 agent 并照抄上一轮真实回复的格式报了个实测值
        assert!(claims_execution("子 agent 跑 sleep(5) 实测约 5013 ms。"));
        assert!(claims_execution("跑出来是 100"));
    }

    #[test]
    fn claims_execution_passes_pasted_output_and_subagent_advice() {
        // 「实测」这一族的收窄取舍：模型引用用户粘来的计时输出时照抄原文形态
        // 「实测耗时: 2005.08 ms」，而编造时说的是「实测约 X」。收前者放行、
        // 拦后者，代价是模型真编「实测耗时 5013 ms」时漏网——两种形态都是猜，
        // 但照抄来自用户输入（可核对），编造来自零凭证轮（无凭证可对）。
        assert!(!claims_execution(
            "这行「实测耗时: 2005.08 ms」说明你的计时脚本跑了一次 sleep(2)。"
        ));
        // 子 agent 只是被讨论时不是执行断言——别名命中也不会进判决，
        // 因为判决要 claims_execution 与凭证缺失同时成立
        assert!(!claims_execution("子 agent 适合并行搜索这类任务。"));
        assert!(!claims_execution(
            "建议先派个子 agent 去探路，主线同时做别的。"
        ));
        // 「返回了 / 结果是」两支刻意不收：代码讲解与算术题里高频
        assert!(!claims_execution("这个函数在空列表时返回了 None。"));
        assert!(!claims_execution("1+1 的结果是 2。"));
    }

    /// 设置页可编辑词表：内置表是默认值，用户这份**全量替换**它。
    ///
    /// 「替换不是合并」是这里的要点——合并的话用户在设置页删词会静默无效，
    /// 而删不掉词等于这个设置没有存在的意义。
    #[test]
    fn claims_execution_uses_configured_keywords_wholesale() {
        let custom = ["我说完了".to_string(), "搞定".to_string()];
        // 自定义词生效
        assert!(claims_execution_with("行了，我说完了。", &custom));
        assert!(claims_execution_with("搞定", &custom));
        // 内置词被替换掉后不再触发：删词 = 该话术的谎报静默放行
        assert!(!claims_execution_with("执行完毕，输出 done", &custom));
        // 空表 = 关掉这一档触发器
        assert!(!claims_execution_with("执行完毕，输出 done", &[]));
        // 空串项不产生「永远命中」的死条目
        assert!(!claims_execution_with(
            "执行完毕，输出 done",
            &[String::new()]
        ));
        // 没配过时的回落值就是内置默认表
        assert_eq!(
            crate::bot::registry::default_execution_claim_patterns().len(),
            27,
            "内置词表条数变了——设置页「恢复默认」与 SPEC 的计数要同步"
        );
        assert!(claims_execution_with(
            "执行完毕，输出 done",
            &crate::bot::registry::default_execution_claim_patterns()
        ));
    }

    #[test]
    fn mentioned_tool_names_maps_chinese_subagent_alias() {
        // 模型写不出 snake_case 的工具名——说「子 agent」，表里是 spawn_subagent。
        // 映射不过去，点名判定会退化成「整条提问零凭证」的粗判。
        assert_eq!(
            crate::bot::registry::mentioned_tool_names("子 agent 跑完了，结果如下"),
            vec!["spawn_subagent"]
        );
        assert_eq!(
            crate::bot::registry::mentioned_tool_names("子代理那边返回了"),
            vec!["spawn_subagent"]
        );
        // 原名照旧，别名不产生重复项
        let mut both = crate::bot::registry::mentioned_tool_names(
            "spawn_subagent 派了个子 agent 去跑 run_python",
        );
        both.sort_unstable();
        assert_eq!(both, vec!["run_python", "spawn_subagent"]);
        // 没提子 agent 时不该凭空带出工具名
        assert!(crate::bot::registry::mentioned_tool_names("都搞定了").is_empty());
    }

    #[test]
    fn claims_execution_passes_code_explanation_without_tool_call() {
        // 零工具调用轮里最常见的正当内容：讲代码 / 讲步骤 / 讲将来时。这批是
        // 扩词表后最可能被误伤的一类，逐条钉住——其中「运行抛出异常时…」是
        // 扩词表当场被测试打回来才砍掉的（见 registry 词表末尾的不收清单）。
        assert!(!claims_execution(
            "这段代码在 `import nosuchmodule_zzz` 处会抛出 ModuleNotFoundError，\
             因为环境里没装这个模块。"
        ));
        assert!(!claims_execution(
            "可以分两步做：第一步安装依赖，第二步运行脚本即可。"
        ));
        assert!(!claims_execution(
            "如果你在这里抛出异常，外层会捕获并继续。"
        ));
        assert!(!claims_execution("运行抛出异常时该脚本会非零退出。"));
        assert!(!claims_execution("任务执行完成后会发通知给你。"));
        assert!(!claims_execution("等运行完成后再刷新页面。"));
        assert!(!claims_execution("每步骤执行完成后记录一条日志。"));
        assert!(!claims_execution("计划分三步：需求分析、开发、测试。"));
        assert!(!claims_execution("按这个计划完成后续两项工作。"));
        assert!(!claims_execution(
            "这段代码会打印 done 吗？不会，它只在导入失败时退出。"
        ));
    }

    #[test]
    fn cap_tool_output_under_cap_unchanged() {
        assert_eq!(cap_tool_output("hello", 30_000), "hello");
        // 恰好等于 cap 也不截
        assert_eq!(cap_tool_output(&"x".repeat(100), 100), "x".repeat(100));
    }

    #[test]
    fn cap_tool_output_keeps_head_and_tail() {
        let text = format!(
            "{}{}{}",
            "H".repeat(20_000),
            "M".repeat(20_000),
            "T".repeat(10_000)
        );
        let out = cap_tool_output(&text, 30_000);
        assert!(out.contains("已按 maxToolOutputBytes=30000 截断，原文 50000 字节"));
        // 头部保留 cap - 6K = 24K（原文头 20K 全 H + 中段 4K M）；尾部保留 6K
        assert!(out.starts_with(&format!("{}{}", "H".repeat(20_000), "M".repeat(4_000))));
        assert!(out.ends_with(&"T".repeat(6_000)));
        // 中段被替身说明取代（头部只含 4K 个 M，连续 5K M 必来自被替身的中段）
        assert!(!out.contains(&"M".repeat(5_000)));
    }

    #[test]
    fn cap_tool_output_small_cap_degrades_tail() {
        // cap < 5×6K：尾部保留退化为 cap/5，头部仍占大头
        let text = "A".repeat(1_000);
        let out = cap_tool_output(&text, 100);
        assert!(
            out.starts_with(&"A".repeat(80)),
            "头应保留 80：{}",
            &out[..90]
        );
        assert!(out.ends_with(&"A".repeat(20)));
        assert!(out.contains("已按 maxToolOutputBytes=100 截断"));
    }

    #[test]
    fn cap_tool_output_counts_bytes_not_chars() {
        // 中文 3 字节/字：50 个汉字 = 150 字节，cap 30 字节只够 8 个字
        let text = "中".repeat(50);
        let out = cap_tool_output(&text, 30);
        assert!(out.contains("原文 150 字节"));
        assert!(out.starts_with(&"中".repeat(8)), "头部应保留 8 个汉字");
        assert!(out.ends_with(&"中".repeat(2)), "尾部应保留 2 个汉字");
    }

    #[test]
    fn cap_tool_output_never_splits_a_multibyte_char() {
        // 切点落在汉字中间时必须回退到字符边界——String 切片切出半个 UTF-8 字符
        // 不是「乱码」，是 panic：整条工具调用直接崩。所以每个 cap 都要试一遍。
        let text = "中".repeat(50);
        for cap in 1..=150 {
            let out = cap_tool_output(&text, cap); // 不 panic 即为一半的通过条件
            let head: String = out.chars().take_while(|c| *c == '中').collect();
            assert_eq!(
                head.len() % 3,
                0,
                "cap={cap} 头部字节数不是 3 的倍数，说明切进了半个汉字"
            );
        }
    }

    #[test]
    fn effective_cap_is_per_tool_budget_when_tighter() {
        // run_python 的 per-tool 预算 8192 比全局 30_000 紧 → 取 8192
        assert_eq!(effective_output_cap("run_python", 30_000), 8_192);
    }

    #[test]
    fn effective_cap_falls_back_to_global_when_budget_is_looser() {
        // read_text_file 的 per-tool 预算 65536 比全局大 → 全局兜底，否则设置页那个
        // 旋钮在长输出工具上就成空设置
        assert_eq!(effective_output_cap("read_text_file", 30_000), 30_000);
        // 不在注册表里的名字（MCP 挂载工具）也走全局
        assert_eq!(effective_output_cap("mcp_unknown", 30_000), 30_000);
    }

    /// 文档类输出（技能正文）不受全局 cap 压制：截掉它不是省上下文，是删任务。
    ///
    /// 实测：57KB 技能正文按 8KB 头尾节选后 86% 丢失，「工作流程（七步法）」
    /// 整节落在丢失的中段，模型不知道流程怎么走 → 自己去 grep 找标题 →
    /// 工具用错 → 技能判死 → 整个回合结束。
    #[test]
    fn effective_cap_keeps_document_output_whole() {
        assert_eq!(effective_output_cap("use_skill", 30_000), 65_536);
        // 即便全局 cap 调得很小，文档类仍按自己的预算走
        assert_eq!(effective_output_cap("use_skill", 1_000), 65_536);
    }

    /// 恰好等于 cap 的输出不得转存：边界是严格大于。等号判成超限会把
    /// 正好压线的输出白白转存一次（多一次 IO + 节选换指针）。
    #[tokio::test]
    async fn mutant_m5_preview_exact_cap_output_is_not_spilled() {
        let _g = crate::overflow::OVERFLOW_TEST_LOCK.lock().await;
        let app = tauri::test::mock_app();
        let root = crate::overflow::root(app.handle());
        let _ = std::fs::remove_dir_all(&root);
        // run_python 的 per-tool 预算 8192（与 effective_cap 既有断言同锚点）
        let exact = "A".repeat(8_192);
        let out = apply_output_cap(app.handle(), Some(30_000), "run_python", exact.clone()).await;
        assert_eq!(out, exact, "恰等于 cap 的输出必须原样回灌，不得转存");

        let over: String =
            apply_output_cap(app.handle(), Some(30_000), "run_python", "B".repeat(8_193)).await;
        assert!(
            over.contains("已**完整**转存到本地文件"),
            "超 cap 一个字节的输出必须换成指针：{}",
            over.chars().take(60).collect::<String>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 转存失败兜底不得返回空：溢出目录建不出来时退回纯头尾截断——
    /// 结果凭空消失等于这轮工具白跑。把溢出根占成文件即可让 spill 失败。
    #[tokio::test]
    async fn mutant_m6_preview_spill_failure_fallback_is_never_empty() {
        let _g = crate::overflow::OVERFLOW_TEST_LOCK.lock().await;
        let app = tauri::test::mock_app();
        let root = crate::overflow::root(app.handle());
        let _ = std::fs::remove_dir_all(&root);
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&root, "占位文件：让 spill 的 create_dir_all 失败").unwrap();
        let out =
            apply_output_cap(app.handle(), Some(30_000), "run_python", "A".repeat(8_193)).await;
        assert!(!out.is_empty(), "兜底路径必须保留节选，不得返回空串");
        assert!(
            out.contains("已按 maxToolOutputBytes=8192 截断"),
            "兜底走纯头尾截断形态：{}",
            out.chars().take(60).collect::<String>()
        );
        assert!(out.starts_with("AAAA"), "头部节选必须在场");
        std::fs::remove_file(&root).unwrap();
    }

    #[test]
    fn preview_keeps_head_tail_and_notice_within_cap() {
        let text: String = "A".repeat(20_000);
        let notice = "指针";
        let out = preview_with_pointer(&text, notice, 8_192);
        // 指针 6 字节 + 分隔 4 字节，剩余 8182 字节对半分 = 4091
        assert!(out.starts_with(&"A".repeat(4_091)), "头部应保留约一半预算");
        assert!(out.ends_with(&"A".repeat(4_091)), "尾部应保留约一半预算");
        assert!(out.contains("指针"), "指针必须在场");
        // 头尾对半分掉剩下的预算；真不变量是「不超过」
        assert!(out.len() <= 8_192, "节选总长 {} 超出 cap", out.len());
    }

    #[test]
    fn preview_survives_notice_longer_than_cap() {
        // 路径长到吃掉整个 cap 时不得 panic，也不得把正文整个挤没成空
        let text = "A".repeat(5_000);
        let notice = "N".repeat(20_000);
        let out = preview_with_pointer(&text, &notice, 1_000);
        assert!(out.contains(&"N".repeat(100)), "指针仍应完整在场");
    }

    #[test]
    fn preview_never_splits_a_multibyte_char() {
        // 节选路径同样按字节切：切点落在汉字中间会 panic，不是乱码
        let text = "中".repeat(5_000);
        let notice = "指针";
        for cap in 1..=3_000 {
            let out = preview_with_pointer(&text, notice, cap); // 不 panic 即通过
            let head: String = out.chars().take_while(|c| *c == '中').collect();
            assert_eq!(head.len() % 3, 0, "cap={cap} 头部切进了半个汉字");
        }
    }

    #[test]
    fn mutating_tools_cover_task_and_file_writes() {
        // 守卫白名单与工具分发保持一致的关键几个
        for t in [
            "delete_task",
            "add_subtask",
            "toggle_subtask",
            "complete_task",
            "edit_task",
            "create_task",
            "link_file_to_task",
        ] {
            assert!(MUTATING_TOOLS().contains(&t), "{t} 应算变更类工具");
        }
        // 纯查询工具不算变更
        for t in [
            "list_tasks",
            "search_tasks",
            "query_single_task",
            "web_search",
            "fetch_url",
        ] {
            assert!(!MUTATING_TOOLS().contains(&t), "{t} 不应算变更类工具");
        }
    }

    // ── mutation_done 按执行结果置位 ──

    /// 构造一个只带 status 的 ToolResult（mutation_succeeded 不看文本了）
    fn tr(status: crate::bot::registry::ToolStatus) -> crate::bot::registry::ToolResult {
        crate::bot::registry::ToolResult {
            text: String::new(),
            refs: Vec::new(),
            status,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }

    #[test]
    fn mutation_succeeded_true_on_real_success() {
        use crate::bot::registry::ToolStatus;
        for name in ["complete_task", "link_file_to_task", "create_word"] {
            assert!(
                mutation_succeeded(name, &tr(ToolStatus::Ok)),
                "{name} 成功落库应算动过手"
            );
            // 带告警的成功也算动过手（做成了，只是有提示）
            assert!(
                mutation_succeeded(name, &tr(ToolStatus::Warn)),
                "{name} 带告警的成功仍算动过手"
            );
        }
    }

    #[test]
    fn mutation_succeeded_false_on_denied_and_error() {
        use crate::bot::registry::ToolStatus;
        // 门禁拦截 / 用户拒绝 / 预算触顶 / 白名单外 —— 工具根本没执行
        assert!(!mutation_succeeded(
            "link_file_to_task",
            &tr(ToolStatus::Denied)
        ));
        // 执行报错（落库失败、磁盘只读等）
        assert!(!mutation_succeeded("create_task", &tr(ToolStatus::Error)));
    }

    #[test]
    fn mutation_succeeded_false_for_readonly_tools() {
        use crate::bot::registry::ToolStatus;
        // 只读工具即使成功也不算变更
        assert!(!mutation_succeeded("list_tasks", &tr(ToolStatus::Ok)));
    }

    /// 变更声称守卫：跑过「能改状态但核验不了凭证」的工具后，如实汇报
    /// 不该被判成凭空汇报。
    ///
    /// 典型误伤：模型用 run_python 写完文件，如实说「已写入 /tmp/a.csv」
    /// ——「已写入」命中 write_file 的声称话术，但 run_python 不在变更类
    /// 工具里，mutation_done 是假的 → 守卫把终稿推回让模型重跑，副作用
    /// 翻倍，末尾还挂一条「这句没有对应凭证」的更正。
    #[test]
    fn claim_mutation_does_not_fire_after_unverifiable_tool_ran() {
        let text = "已写入 /tmp/a.csv";
        assert!(
            claims_mutation(text),
            "「已写入」确实是变更声称话术（对照组）"
        );
        assert!(
            claim_mutation_fires(false, false, text),
            "什么都没动过却声称写入 → 该拦"
        );
        assert!(
            !claim_mutation_fires(false, true, text),
            "run_python 写过盘 → 「没凭证」推不出「没动手」，不该拦"
        );
        // 变更类工具成功过 → 同样不该拦
        assert!(!claim_mutation_fires(true, false, text));
    }

    /// 守卫仍要拦得住真正的凭空汇报：不可核验工具没跑过时照拦。
    #[test]
    fn claim_mutation_still_fires_without_any_receipt() {
        assert!(claim_mutation_fires(false, false, "已把任务移至回收站"));
        // 没有变更声称 → 不拦
        assert!(!claim_mutation_fires(false, false, "今天天气不错"));
    }
}

#[cfg(test)]
mod stop_guard_tests {
    use super::*;

    /// `/stop` 必须覆盖主循环里两处长 await：
    /// - 终稿自检的侧调（`summarize_http`，单次最多 60s）；
    /// - 重规划（`replan`，单次最多 60s）。
    ///
    /// 两处都不看停止信号时，用户点了停止后 UI 仍停在「收尾中」最长两分钟，
    /// 看着像停止失灵。
    ///
    /// 这两处在主循环深处，行为级验证得用 mock server 跑完整轮次、并在恰当
    /// 的时机置位停止——成本远高于收益，故扫源码钉住「调用点之前有 stop
    /// 检查，且检查与调用之间没有别的 await」。
    #[test]
    fn stop_guards_the_two_long_awaits() {
        for (what, needle) in [
            ("终稿自检侧调", "crate::bot_chat::summarize_http("),
            ("重规划", "replan(snapshot, reason).await"),
        ] {
            // 按调用点定位所在文件再扫——stop 检查与调用点的邻接关系只在
            // 同一个文件里成立，跨文件拼接会让邻接断言失去意义。
            let src = prod_file_with(needle);
            let idx = src
                .find(needle)
                .unwrap_or_else(|| panic!("应能找到{what}的调用点"));
            // 在整段前缀里找最后一次 stop 检查（rfind 返回的一定是字符边界，
            // 按字节切窗口会切在多字节字符中间）
            let last_stop = src[..idx]
                .rfind("stop.stopped()")
                .unwrap_or_else(|| panic!("{what}之前必须有 stop 检查（否则 /stop 后还能烧 60s）"));
            assert!(
                idx - last_stop <= 4000,
                "{what}的 stop 检查离调用点太远（{} 字节），中间可能夹着别的 await",
                idx - last_stop
            );
            let between = &src[last_stop..idx];
            assert!(
                !between.contains(".await"),
                "{what}的 stop 检查与调用点之间不得夹着别的 await（检查会形同虚设）"
            );
        }
    }
}

#[cfg(test)]
mod rounds_fuse_tests {
    use super::*;

    /// 轮数熔断的出口必须带回熔断前的正文，且标记可被子任务侧识别。
    ///
    /// 回归背景：子任务撞满 20 轮时熔断走裸 Err，连同最后一轮 streamed 正文一起
    /// 丢弃，子任务侧收不到收尾 JSON → result_json 写 NULL → 父 agent 回灌拿不到
    /// summary，只能从零重跑（设计 §6「部分结果不丢弃」当时没兑现）。
    #[test]
    fn rounds_fuse_message_keeps_partial_text() {
        let msg = rounds_fuse_message("已查到 det.onnx / rec.onnx 在 pp-ocr-v6/ 下", "", 20);
        assert!(
            msg.contains("已查到 det.onnx"),
            "熔断必须保住熔断前正文：{msg}"
        );
        assert!(
            msg.contains(ROUNDS_FUSE_MARKER),
            "熔断正文须带子任务侧识别的标记：{msg}"
        );
        assert!(msg.contains("20 轮上限"), "熔断消息应带上限值：{msg}");
        // 子任务侧按 rounds 标记判 max_turns，所以单轮 Function 熔断的正文绝不能
        // 带这个标记——否则「工具调用超限」会被误报成「轮数超限」。
        assert!(
            !fuse_message("x", "", 100).contains(ROUNDS_FUSE_MARKER),
            "单轮 Function 熔断不得带轮数标记"
        );
    }

    /// 轮数软警告阈值与单轮软警告同口径：小 cap 不塌成 0，且留得出收尾轮次。
    #[test]
    fn rounds_warn_threshold_leaves_room_to_finish() {
        let at =
            |max_rounds: usize| (max_rounds * 7 / 10).clamp(1, max_rounds.saturating_sub(1).max(1));
        // 子 agent 典型预算 20 → 第 14 轮起提醒，剩 6 轮够写收尾 JSON
        assert_eq!(at(20), 14);
        assert!(at(20) < 20, "阈值必须早于熔断，否则预警没有意义");
        // 小 cap 整除塌到 0 时钳回 1（否则提示变成「已进行 0 轮」）；max_rounds=3
        // 的 7/10 是 2，留 1 轮收尾——阈值贴着熔断正是小预算的固有代价
        assert_eq!(at(3), 2);
        assert_eq!(at(1), 1);
        // 主聊天默认 50 同样留 15 轮
        assert_eq!(at(50), 35);
    }

    /// 「轮」的口径必须由循环自报，不能数 `llm.request` 事件。
    ///
    /// 回归背景：一轮里可能网络重试、可能撞 max_tokens 自动续写、可能补终稿自检，
    /// 数事件必然把上报轮数算大——上报值与真正约束循环的 max_rounds 对不上，
    /// trace 里的 turns 就成了「请求数」而非「轮数」。
    #[test]
    fn turn_count_is_rounds_not_llm_requests() {
        // 口径契约：LoopTrace 同时带轮数与请求数两个字段，二者不可复用同一来源。
        let t = LoopTrace::default();
        assert_eq!(t.turn_count, 0);
        assert_eq!(t.request_count, 0);
        // 审计闭包只给请求数加一（数据源是 llm.request 事件）；
        // 轮数由 core 的 on_round 单独写。测试锚定这条分工：请求数归请求数、
        // 轮数归轮数，谁把两者混起来就是回归。
        let src = prod_src();
        assert!(
            src.contains("if event == \"llm.request\"")
                && src.contains("request_for_audit.fetch_add"),
            "请求数仍应数 llm.request 事件"
        );
        assert!(
            src.contains("turn_count: rounds_used.load"),
            "turn_count 必须来自 rounds_used（循环自报轮数），不得回到数请求事件"
        );
        assert!(
            src.contains("request_count: request_count.load"),
            "请求数应单独落在 request_count 字段"
        );
    }

    #[test]
    fn skill_frontmatter_max_rounds_overrides_default() {
        // Skill frontmatter 声明 max_rounds: 25 → 生效，run_model_loop 收到 25
        let meta = crate::bot_skills::parse_meta(
            "---\nname: minimax-docx\nmax_rounds: 25\n---\n# body\n",
            "minimax-docx",
        );
        assert_eq!(meta.max_rounds, Some(25));
        assert_eq!(resolve_max_rounds(meta.max_rounds, 50), 25);
    }

    #[test]
    fn skill_without_max_rounds_falls_back_to_default_50() {
        // 现有 Skill 未声明 max_rounds → None → fallback（：fallback = 配置解析值，测试传默认 50）
        let meta = crate::bot_skills::parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert_eq!(meta.max_rounds, None);
        assert_eq!(
            resolve_max_rounds(meta.max_rounds, DEFAULT_MAX_ROUNDS),
            DEFAULT_MAX_ROUNDS
        );
        assert_eq!(DEFAULT_MAX_ROUNDS, 50);
    }

    #[test]
    fn fuse_trips_at_cap_boundary() {
        // MAX_FUNCTION_CALLS_PER_REQUEST = 50 实际生效：模拟主循环计数，
        // 构造 101 个 tool_calls → 第 101 次触发熔断默认上限 50→100）
        assert_eq!(MAX_FUNCTION_CALLS_PER_REQUEST, 100);
        let mut calls = 0usize;
        let mut fused_msg: Option<String> = None;
        for _ in 0..101 {
            calls += 1;
            if should_fuse(calls, MAX_FUNCTION_CALLS_PER_REQUEST) {
                fused_msg = Some(fuse_message("前文", "", MAX_FUNCTION_CALLS_PER_REQUEST));
                break;
            }
        }
        let msg = fused_msg.expect("101 次 Function 调用内必须触发熔断");
        assert!(msg.contains("⏹ 已熔断"), "熔断消息应含「⏹ 已熔断」：{msg}");
        assert!(msg.contains("100 次上限"), "熔断消息应带上限值：{msg}");
        // 配置覆盖：maxFunctionCalls=30 → 第 31 次熔断（派生链第二级）
        // 边界：默认 100 时第 100 次放行、第 101 次熔断
        assert!(!should_fuse(100, MAX_FUNCTION_CALLS_PER_REQUEST));
        assert!(should_fuse(101, MAX_FUNCTION_CALLS_PER_REQUEST));
        let fused = (1..=31).any(|n| should_fuse(n, 30));
        assert!(fused, "cap=30 时第 31 次必须熔断");
        // 边界：第 50 次放行，第 51 次熔断
        assert!(!should_fuse(50, 50));
        assert!(should_fuse(51, 50));
    }

    /// 熔断上限只在一处钳：上游传进来的越界值必须由派生链末端收口。
    ///
    /// 钳位曾同时存在于薄壳（组装 deps 时）与 core 派生链末端，两份边界改一处
    /// 漏一处不会有任何编译或测试信号。抽成 fuse_cap_for 后，薄壳原样透传
    /// （9999 进来，末端才钳），本测试钉住这条契约。
    #[test]
    fn fuse_cap_clamps_at_the_only_consumer() {
        let cap = fuse_cap_upper_bound();
        assert_eq!(fuse_cap_for(None, None), MAX_FUNCTION_CALLS_PER_REQUEST);
        assert_eq!(
            fuse_cap_for(None, Some(9_999)),
            cap,
            "上游未钳时，末端必须收口到上界"
        );
        assert_eq!(fuse_cap_for(None, Some(0)), 1, "0 次调用无意义，钳到 1");
        assert_eq!(fuse_cap_for(None, Some(30)), 30, "区间内原样放行");
        // 子 agent 行上预算优先于全局覆盖，且同样受末端钳制约束；预算之上
        // 恒加收尾裕量（预算触顶后 dispatch 只拒读放行写，熔断退为兜底）
        assert_eq!(
            fuse_cap_for(Some(7), Some(9_999)),
            7 + SUBAGENT_WRAP_UP_FUSE_MARGIN
        );
        assert_eq!(fuse_cap_for(Some(u32::MAX), None), cap);
        // 漂移守卫：同一条边界在本文件里只允许有一份。薄壳那份钳位被删掉后，
        // 生产代码里不应再出现 `clamp(1, 500)` 字面量——它是当年人手抄的副本，
        // 被绕过也不报错（单测直驱 core 时根本不经薄壳）。
        let production = prod_src();
        assert!(
            !production.contains("clamp(1, 500)"),
            "熔断上界不得在本文件内再存一份字面量副本（边界单源在 params）"
        );
        assert!(
            production.matches("fuse_cap_upper_bound()").count() >= 2,
            "熔断钳位必须走上界投影（定义 + 至少一处调用），不得另写数字"
        );
    }
}

/// L6 接线守卫：`post_turn` 钩子层必须真被模型循环调到，且排在
/// 「答案定稿」之后、「回灌与返回」之前。
///
/// 与 dispatch 里 L5 那条同款理由：注册表的行为测试验不到调用点是否存在。
/// 变异实测把模型循环里那段调用删掉（最终回复再也不过钩子）时，全仓测试
/// 照样全绿——所以这里钉「语句存在 + 两点相对位置」。
///
/// 扫描源是 prod_src()（门面 + 全部子模块的生产代码，不含测试文件）：
/// 拆分后生产调用分布在多个文件里，只扫门面会漏；而把测试文件也拼进来会让
/// 守卫自匹配——守卫要找的字符串字面量就在测试文件里，生产调用真被删掉后
/// find 仍会命中它，测试依旧绿，那才是真的假保证。
#[cfg(test)]
mod post_turn_layer_tests {
    use super::*;

    #[test]
    fn post_turn_layer_is_wired_after_answer_is_final() {
        let production = prod_src();
        assert_eq!(
            production.matches("run_post_turn(").count(),
            1,
            "生产代码里 post_turn 调用应恰好 1 处（多处说明接线分叉了）"
        );
        let hook = production
            .find("run_post_turn(")
            .expect("模型循环必须调用 post_turn 钩子层");
        let answer = production
            .find("let mut answer = if length_prefix.is_empty()")
            .expect("找不到答案定稿点");
        let ret = production
            .find("return Ok((answer, collected_refs));")
            .expect("找不到主出口");
        assert!(
            answer < hook,
            "钩子必须在答案定稿之后（改写的要是用户真会看到的那一份）"
        );
        assert!(hook < ret, "钩子必须在返回之前：否则改写不会到达用户");
        // 返回值必须被消费：run_post_turn 跑了却把结果丢掉，等于钩子改写
        // answer 之后的失败既没人看见也没人记，只钉调用存在是不够的
        let tail = &production[hook..];
        let consumed = tail
            .find("run.failures")
            .expect("run_post_turn 的返回值必须被读（至少 failures 列表）");
        let ret_after_hook = tail
            .find("return Ok((answer, collected_refs));")
            .expect("找不到主出口");
        assert!(
            consumed < ret_after_hook,
            "钩子失败的消费必须在返回之前，否则失败静默丢失"
        );
    }
}
