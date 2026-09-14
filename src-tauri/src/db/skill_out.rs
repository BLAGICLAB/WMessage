//! Skill 执行结果持久化

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSkillOutcome {
    pub skill_name: String,
    pub kind: String,
    pub reason: Option<String>,
    pub completed_summary: Option<String>,
    pub rollback_attempted: Option<bool>,
    pub last_at_ms: i64,
}

pub fn upsert_skill_outcome(
    conn: &rusqlite::Connection,
    o: &PersistedSkillOutcome,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO skill_outcomes (skill_name, kind, reason, completed_summary, rollback_attempted, last_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![o.skill_name, o.kind, o.reason, o.completed_summary, o.rollback_attempted.map(|b| if b { 1 } else { 0 }), o.last_at_ms],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_all_skill_outcomes(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, PersistedSkillOutcome>, String> {
    let mut stmt = conn.prepare("SELECT skill_name, kind, reason, completed_summary, rollback_attempted, last_at_ms FROM skill_outcomes").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PersistedSkillOutcome {
                skill_name: r.get(0)?,
                kind: r.get(1)?,
                reason: r.get(2)?,
                completed_summary: r.get(3)?,
                rollback_attempted: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
                last_at_ms: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let o = row.map_err(|e| e.to_string())?;
        map.insert(o.skill_name.clone(), o);
    }
    Ok(map)
}
