//! Numeric attention parameters only; prose is never expired by a timer.
use crate::data::{AppResult, Database};
use serde_json::{json, Value};
pub const LAYERS: [&str; 3] = ["long_term", "medium_term", "short_term"];
pub fn defaults(now: i64) -> Value {
    json!({"long_term":{"stickiness":0.95,"strength":60.0,"decay_per_hour":0.05},"medium_term":{"stickiness":0.7,"strength":60.0,"decay_per_hour":0.5},"short_term":{"stickiness":0.2,"strength":60.0,"decay_per_hour":5.0},"updated_at":now,"reviewed_revision":""})
}
pub fn normalize(raw: Value, now: i64) -> Value {
    if LAYERS
        .iter()
        .all(|layer| raw[layer]["strength"].as_f64().is_some())
    {
        return raw;
    }
    let mut next = defaults(now);
    // Preserve the old numeric short-term strength; legacy status cannot expire prose.
    if let Some(score) = raw["score"].as_f64() {
        next["short_term"]["strength"] = json!(score.clamp(0.0, 100.0));
    }
    next
}
pub fn view(db: &Database) -> AppResult<Value> {
    let raw: String = db
        .lock()
        .query_row("SELECT payload FROM attention WHERE id=1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(normalize(
        serde_json::from_str(&raw).map_err(|e| e.to_string())?,
        crate::data::now(),
    ))
}
/// Content fingerprint excludes fetch timestamps so refreshing an unchanged feed is not novelty.
pub fn world_revision(db: &Database) -> u64 {
    use std::hash::{Hash, Hasher};
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for field in ["fetched_at", "updated_at", "now_ms"] {
                    map.remove(field);
                }
                for v in map.values_mut() {
                    strip(v);
                }
            }
            Value::Array(items) => {
                for v in items {
                    strip(v);
                }
            }
            _ => {}
        }
    }
    let conn = db.lock();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for query in [
        "SELECT payload FROM daily_world WHERE id=1",
        "SELECT payload FROM hourly_world WHERE id=1",
        "SELECT payload FROM github_world WHERE id=1",
    ] {
        if let Ok(raw) = conn.query_row(query, [], |r| r.get::<_, String>(0)) {
            if let Ok(mut value) = serde_json::from_str::<Value>(&raw) {
                strip(&mut value);
                value.to_string().hash(&mut hash);
            }
        }
    }
    hash.finish()
}

pub fn reduce(mut parameters: Value, now: i64) -> Value {
    let last = parameters["updated_at"].as_i64().unwrap_or(now);
    let hours = now.saturating_sub(last).max(0) as f64 / 3_600_000.0;
    for layer in LAYERS {
        let strength = parameters[layer]["strength"].as_f64().unwrap_or(60.0);
        let decay = parameters[layer]["decay_per_hour"].as_f64().unwrap_or(0.0);
        parameters[layer]["strength"] = json!((strength - hours * decay).clamp(0.0, 100.0));
    }
    parameters["updated_at"] = json!(now.max(last));
    parameters
}
pub fn reinforce(mut parameters: Value, signals: &Value, revision: &str) -> Value {
    if parameters["reviewed_revision"] == revision {
        return parameters;
    }
    for layer in LAYERS {
        let delta = match signals[layer].as_str() {
            Some("strengthen") => 12.0,
            Some("ease") => -12.0,
            _ => 0.0,
        };
        let stickiness = parameters[layer]["stickiness"].as_f64().unwrap_or(0.0);
        let strength = parameters[layer]["strength"].as_f64().unwrap_or(60.0);
        parameters[layer]["strength"] =
            json!((strength + delta * (1.0 - stickiness)).clamp(0.0, 100.0));
    }
    parameters["reviewed_revision"] = json!(revision);
    parameters
}
pub fn advance(db: &Database, now: i64) -> AppResult<()> {
    let mut conn = db.lock();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let raw: String = tx
        .query_row("SELECT payload FROM attention WHERE id=1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let next = reduce(
        normalize(serde_json::from_str(&raw).map_err(|e| e.to_string())?, now),
        now,
    );
    tx.execute(
        "UPDATE attention SET payload=? WHERE id=1",
        [next.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let raw: String = tx
        .query_row("SELECT payload FROM agent_drive WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let old: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let prose = crate::drive::normalize(old.clone());
    if prose != old {
        tx.execute(
            "UPDATE agent_drive SET payload=?,reviewed_event='' WHERE id=1",
            [prose.to_string()],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn different_timescales_decay_without_lifecycle_and_duplicate_reinforcement() {
        let p = defaults(1000);
        let decayed = reduce(p, 3_601_000);
        assert_eq!(decayed["short_term"]["strength"], 55.0);
        assert_eq!(decayed["medium_term"]["strength"], 59.5);
        assert_eq!(decayed["long_term"]["strength"], 59.95);
        assert_eq!(reduce(decayed.clone(), 0), decayed);
        let signals =
            json!({"long_term":"strengthen","medium_term":"strengthen","short_term":"strengthen"});
        let boosted = reinforce(decayed, &signals, "revision-1");
        assert_eq!(reinforce(boosted.clone(), &signals, "revision-1"), boosted);
        let exhausted = reduce(boosted, 1_000_000_000_000);
        assert_eq!(exhausted["short_term"]["strength"], 0.0);
        assert!(exhausted.get("status").is_none());
    }
}
