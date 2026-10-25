use super::{mentioned_tool_names, TOOLS_TABLE};

/// 点名扫描按标识符整词匹配，前缀名对不互相污染。
///
/// 复现的是实机可判的假指控：模型真调了 `create_word_revisions`（Word 润色的
/// 推荐路径，prompts/system.rs 规则 10 明写），终稿报「create_word_revisions
/// 执行完毕」——纯子串扫描会多点出 `create_word`，凭证表查不到，判决成
/// 「你刚才说调了 create_word 并给出了结果，但没有任何 create_word 的成功
/// 调用记录——这个结果是编造的」，返工指令再叫它去真调 create_word，
/// 产出一份非修订模式的 docx。为一次假指控付一次错误产物。
#[test]
fn prefix_tool_name_is_not_matched_inside_longer_name() {
    assert_eq!(
        mentioned_tool_names("create_word_revisions 执行完毕，返回 3 个修订版本。"),
        vec!["create_word_revisions"],
        "提到 create_word_revisions 时不得同时点名 create_word"
    );
    assert_eq!(
        mentioned_tool_names("我跑完了 create_word_revisions，修订已写回原文档。"),
        vec!["create_word_revisions"]
    );
    // 反向：纯 create_word 仍要点名得出来（中文紧贴标识符是正常写法）
    assert_eq!(
        mentioned_tool_names("create_word 执行完毕，产物在 AI_Gen_Files/。"),
        vec!["create_word"]
    );
    assert_eq!(
        mentioned_tool_names("已用 create_word 生成，工具返回 3 处改动。"),
        vec!["create_word"]
    );
}

/// 整词匹配不能太紧：中文紧贴标识符、以及句末标点都算命中。
#[test]
fn chinese_adjacent_and_trailing_punctuation_still_match() {
    assert_eq!(
        mentioned_tool_names("我用 run_python 跑了一遍"),
        vec!["run_python"]
    );
    assert_eq!(
        mentioned_tool_names("跑完了 run_python。"),
        vec!["run_python"]
    );
    assert_eq!(mentioned_tool_names("（run_python）"), vec!["run_python"]);
    // 标识符内部续字符不算点名：没有 run_python2 这个工具，报它等于点名一个
    // 查不到凭证的假名字
    assert!(mentioned_tool_names("跑完了 run_python2。").is_empty());
}

/// 前缀名对必须互不点名——守的是**性质**而不是「表里不许有前缀名对」。
///
/// 表里确实有 `create_word` ⊂ `create_word_revisions`（重命名要动提示词、
/// schema、dispatch 三处，不值当）。要守的是整词匹配这个性质：只要点名扫描
/// 按标识符边界匹配，前缀名对就永远各自独立；一旦有人把它退回纯子串，这条
/// 立刻红——而不是等到某次实机偶发假指控才被发现。
#[test]
fn prefix_name_pairs_never_mention_each_other() {
    let names: Vec<&str> = TOOLS_TABLE.iter().map(|t| t.name).collect();
    let mut pairs = 0;
    for longer in &names {
        let expected = mentioned_tool_names(longer);
        for shorter in &names {
            if shorter != longer && longer.starts_with(shorter) {
                pairs += 1;
                assert!(
                    !expected.contains(shorter),
                    "点名 {longer} 时不该同时点名它的前缀 {shorter}：纯子串匹配已回来"
                );
                assert!(expected.contains(longer), "点名 {longer} 时必须认出它自己");
            }
        }
    }
    assert!(
        pairs > 0,
        "表里没有前缀名对了，本测试失去意义——留给下一条前缀关系出现时用"
    );
}
