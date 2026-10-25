use super::*;

fn sample_params() -> WordTemplateParams {
    let style = |align: &str, indent: f64| WordTextStyle {
        font_east: "仿宋".into(),
        font_ascii: "仿宋".into(),
        size_half: 32,
        bold: false,
        align: align.into(),
        line_rule: "auto".into(),
        line_line: 360,
        indent_chars: indent,
        before: 0,
        after: 0,
    };
    WordTemplateParams {
        version: 1,
        title: style("center", 0.0),
        body: style("both", 2.0),
        h1: style("left", 0.0),
        h2: style("left", 0.0),
        h3: style("left", 0.0),
    }
}

#[test]
fn params_serde_roundtrip_snake_keys() {
    // 旁车 json / 提取脚本 stdout / 生成入参三方同键（snake_case），
    // 字段名改写会让任何一方静默拿默认值
    let p = sample_params();
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"font_east\""), "键名不得改写：{s}");
    assert!(s.contains("\"indent_chars\""), "键名不得改写：{s}");
    let back: WordTemplateParams = serde_json::from_str(&s).unwrap();
    assert_eq!(back, p);
}

#[test]
fn update_renames_docx_json_and_default_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.docx"), b"x").unwrap();
    std::fs::write(template_params_path_in(dir, "a"), "{}").unwrap();
    std::fs::write(default_marker_path(dir), "a").unwrap();

    word_template_params_update_in(dir, "a", "b", &sample_params()).unwrap();

    assert!(dir.join("b.docx").exists(), "docx 未改名");
    assert!(!dir.join("a.docx").exists());
    assert!(dir.join("b.json").exists(), "旁车参数未随新名落盘");
    assert!(!template_params_path_in(dir, "a").exists(), "旧旁车残留");
    assert_eq!(
        std::fs::read_to_string(default_marker_path(dir)).unwrap(),
        "b",
        "_default 标记未连带"
    );
    // 落盘的参数可读回且保真
    assert_eq!(
        word_template_params_load_in(dir, "b").unwrap(),
        sample_params()
    );
}

#[test]
fn update_without_rename_keeps_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.docx"), b"x").unwrap();
    let mut params = sample_params();
    params.body.size_half = 28;
    word_template_params_update_in(dir, "a", "a", &params).unwrap();
    assert!(dir.join("a.docx").exists());
    assert_eq!(
        word_template_params_load_in(dir, "a")
            .unwrap()
            .body
            .size_half,
        28
    );
}

#[test]
fn update_rejects_bad_input() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.docx"), b"x").unwrap();
    std::fs::write(dir.join("b.docx"), b"x").unwrap();
    // 空名 / 非法字符 / 不存在的旧模板 / 同名冲突
    assert!(word_template_params_update_in(dir, "a", "  ", &sample_params()).is_err());
    assert!(word_template_params_update_in(dir, "a", "x/y", &sample_params()).is_err());
    assert!(word_template_params_update_in(dir, "a", "..x", &sample_params()).is_err());
    assert!(word_template_params_update_in(dir, "nope", "c", &sample_params()).is_err());
    assert!(word_template_params_update_in(dir, "a", "b", &sample_params()).is_err());
    // 上面全部失败：文件未被误动
    assert!(dir.join("a.docx").exists() && dir.join("b.docx").exists());
}
