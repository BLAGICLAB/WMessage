use super::TaskStatus;

#[test]
fn as_str_matches_wire_format() {
    assert_eq!(TaskStatus::Todo.as_str(), "todo");
    assert_eq!(TaskStatus::Doing.as_str(), "doing");
    assert_eq!(TaskStatus::Done.as_str(), "done");
}

#[test]
fn display_matches_as_str() {
    for s in TaskStatus::ALL {
        assert_eq!(s.to_string(), s.as_str());
    }
}

#[test]
fn from_str_accepts_valid_lowercase() {
    assert_eq!("todo".parse::<TaskStatus>().unwrap(), TaskStatus::Todo);
    assert_eq!("doing".parse::<TaskStatus>().unwrap(), TaskStatus::Doing);
    assert_eq!("done".parse::<TaskStatus>().unwrap(), TaskStatus::Done);
}

/// wire format 是大小写敏感的裸小写字符串(前端 `ColumnId` 契约)
#[test]
fn from_str_rejects_invalid_and_case_variants() {
    for bad in ["", "TODO", "Todo", "doing ", " archived", "archived", "0"] {
        assert!(bad.parse::<TaskStatus>().is_err(), "{bad:?} 不应被接受");
    }
}

/// 锁住「与改 enum 前的 String wire format 字节级兼容」这一契约
#[test]
fn serde_round_trips_wire_string() {
    assert_eq!(
        serde_json::from_str::<TaskStatus>("\"doing\"").unwrap(),
        TaskStatus::Doing
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Done).unwrap(),
        "\"done\""
    );
}

#[test]
fn serde_rejects_unknown_variant() {
    assert!(serde_json::from_str::<TaskStatus>("\"archived\"").is_err());
}

#[test]
fn all_lists_three_variants_in_declared_order() {
    assert_eq!(
        TaskStatus::ALL,
        &[TaskStatus::Todo, TaskStatus::Doing, TaskStatus::Done]
    );
}
