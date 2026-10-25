// 回归锁（读源字符串，同 bot_py 对 runtime.rs 的锁法）：
// 五个生成函数都必须带「退出码 0 ≠ 文件落盘」真校验——脚本静默跳过保存的
// 路径会假成功（杜绝对用户谎报已生成）。新增 doc_make_* 时必须同步补齐块，
// 否则此测试红。

#[test]
fn fake_success_guard_covers_all_doc_makers() {
    let src = include_str!("../document.rs");
    for name in [
        "doc_make_word",
        "doc_make_word_revisions",
        "doc_make_excel",
        "doc_make_pdf",
        "doc_make_ppt",
    ] {
        let marker = format!("{name} 假成功拦截");
        assert!(src.contains(&marker), "{name} 缺少假成功拦截块");
    }
}

/// 模板参数化回归锁：收割启发式单点在 HARVEST_PROTO_SCRIPT（上传/编辑
/// 面板/生成自愈三处共用），MAKE_DOCX_SCRIPT 只按 tpl_params 构造格式——
/// 启发式回流生成脚本会造成提取与生成两套口径漂移（用户模板实测过：
/// 提取对了、生成又走别的路，产物与参数对不上）
#[test]
fn docx_template_params_single_source() {
    let make = super::MAKE_DOCX_SCRIPT;
    let harvest = super::HARVEST_PROTO_SCRIPT;
    for marker in [
        "def proto_of",
        "def merge_layer",
        "has_first_line_ind",
        "def style_of",
    ] {
        assert!(harvest.contains(marker), "提取脚本缺启发式 {marker}");
    }
    for marker in ["tpl_params", "def mk_rpr", "def mk_ppr"] {
        assert!(make.contains(marker), "生成脚本缺参数构造 {marker}");
    }
    // 针线拼接写法：字面量整串写会与脚本正文里的同名字段自命中
    assert!(
        !make.contains(concat!("proto", "_of")),
        "收割启发式回流生成脚本（两套口径漂移）"
    );
    // 参数构造按 schema 序插 pPr/rPr 首子元素；标题补 outlineLvl 导航层级
    assert!(
        make.contains("para._p.insert(0, mk_ppr("),
        "pPr 未按首子元素插入"
    );
    assert!(
        make.contains("run._element.insert(0, mk_rpr("),
        "rPr 未按首子元素插入"
    );
    assert!(make.contains("outlineLvl"), "生成脚本缺 outlineLvl");
}
