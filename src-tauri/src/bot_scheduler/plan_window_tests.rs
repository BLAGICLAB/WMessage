use super::*;
use chrono::TimeZone;

fn at(h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
    chrono::Local
        .with_ymd_and_hms(2026, 10, 14, h, mi, 0)
        .single()
        .expect("valid local time")
}

#[test]
fn in_window_snaps_and_sets_30min_block() {
    // 09:07 → 吸附 09:00–09:30
    let (s, e) = scheduled_plan_window(at(9, 7)).unwrap();
    assert_eq!(s, "2026-10-14T09:00");
    assert_eq!(e, "2026-10-14T09:30");
    // 17:50 → 吸附 17:30–18:00（终点恰在窗口边界，算入）
    let (s, e) = scheduled_plan_window(at(17, 50)).unwrap();
    assert_eq!(s, "2026-10-14T17:30");
    assert_eq!(e, "2026-10-14T18:00");
    // 08:00 整点即窗口起点
    assert!(scheduled_plan_window(at(8, 0)).is_some());
}

#[test]
fn out_of_window_returns_none() {
    // 07:50 → 吸附 07:30，早于窗口起点 08:00
    assert!(scheduled_plan_window(at(7, 50)).is_none());
    // 18:05 → 吸附 18:00，终点 18:30 越出窗口终点
    assert!(scheduled_plan_window(at(18, 5)).is_none());
    // 凌晨 / 深夜作业（窗口外不设排期，照旧建卡+执行）
    assert!(scheduled_plan_window(at(0, 15)).is_none());
    assert!(scheduled_plan_window(at(22, 0)).is_none());
}

#[test]
fn format_matches_build_plan_dt() {
    let (s, e) = scheduled_plan_window(at(9, 7)).unwrap();
    // 与前端 buildPlanDT 同格式：YYYY-MM-DDTHH:mm
    assert_eq!(s.len(), 16);
    assert_eq!(&s[4..5], "-");
    assert_eq!(&s[10..11], "T");
    assert_eq!(e.len(), 16);
}
