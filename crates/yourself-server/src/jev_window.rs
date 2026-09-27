//! One preflight window for every Jev request; history eviction never mutates durable memory.
use crate::data::{self, AppResult, Database};
use rusqlite::params;
use serde_json::{json, Value};
use std::hash::{Hash, Hasher};
pub const MAX_BYTES: usize = 24_576;
#[derive(Clone)]
struct Entry {
    path: Vec<String>,
    index: usize,
    key: String,
    hits: i64,
    touched: i64,
}
fn candidates(value: &Value, path: &mut Vec<String>, out: &mut Vec<Entry>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                path.push(key.clone());
                candidates(value, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            let name = path.last().map(String::as_str).unwrap_or("");
            // Never prune question-indexed candidates, tool arguments, current event, or goal proposal.
            let history = matches!(
                name,
                "focus_memories"
                    | "available_skills"
                    | "retrieved_memories"
                    | "recent_interactions"
                    | "recent_work"
                    | "recent_tool_results_untrusted"
                    | "headlines"
                    | "topics"
                    | "projects"
                    | "recent_actions"
                    | "recent_contact_history"
                    | "conversation"
            );
            if history {
                let keep = if matches!(
                    name,
                    "recent_interactions" | "conversation" | "recent_contact_history"
                ) {
                    2
                } else {
                    0
                };
                for (index, item) in items
                    .iter()
                    .enumerate()
                    .take(items.len().saturating_sub(keep))
                {
                    if name == "conversation" && item["role"] == "system" {
                        continue;
                    }
                    let mut hash = std::collections::hash_map::DefaultHasher::new();
                    path.hash(&mut hash);
                    item.to_string().hash(&mut hash);
                    out.push(Entry {
                        path: path.clone(),
                        index,
                        key: format!("{:016x}", hash.finish()),
                        hits: 0,
                        touched: 0,
                    });
                }
            }
        }
        _ => {}
    }
}
fn remove(state: &mut Value, entry: &Entry) {
    let mut value = state;
    for part in &entry.path {
        value = &mut value[part];
    }
    if let Some(items) = value.as_array_mut() {
        items.remove(entry.index);
    }
}
fn bytes(state: &Value, questions: &Value) -> usize {
    json!({"model":"jev","state":state,"questions":questions})
        .to_string()
        .len()
}
fn project(value: &mut Value, cap: usize, depth: usize) {
    if depth > 12 {
        *value = json!("[omitted deep observation]");
        return;
    }
    match value {
        Value::String(s) if s.len() > cap => {
            let original = s.len();
            let mut boundary = cap.min(s.len());
            while !s.is_char_boundary(boundary) {
                boundary -= 1;
            }
            *s = format!(
                "{} [excerpt; original {original} bytes; omitted content is unknown]",
                &s[..boundary]
            );
        }
        Value::Array(items) => {
            items.truncate(32);
            for item in items {
                project(item, cap, depth + 1);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if !matches!(
                    key.as_str(),
                    "exact_arguments" | "tool_args" | "proposed_prose"
                ) {
                    project(item, cap, depth + 1);
                }
            }
        }
        _ => {}
    }
}
pub fn prepare(
    db: &Database,
    mut state: Value,
    questions: &Value,
    purpose: &str,
) -> AppResult<Value> {
    let before = bytes(&state, questions);
    let mut entries = vec![];
    candidates(&state, &mut vec![], &mut entries);
    let mut conn = db.lock();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // Metadata contains hashes only, never copies of private context.
    tx.execute_batch("CREATE TABLE IF NOT EXISTS jev_lfu(purpose TEXT NOT NULL,key TEXT NOT NULL,hits INTEGER NOT NULL,touched INTEGER NOT NULL,PRIMARY KEY(purpose,key));").map_err(|e|e.to_string())?;
    for entry in &mut entries {
        tx.execute("INSERT INTO jev_lfu VALUES(?,?,1,?) ON CONFLICT(purpose,key) DO UPDATE SET hits=hits+1",params![purpose,entry.key,data::now()]).map_err(|e|e.to_string())?;
        (entry.hits, entry.touched) = tx
            .query_row(
                "SELECT hits,touched FROM jev_lfu WHERE purpose=? AND key=?",
                params![purpose, entry.key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
    }
    entries.sort_by(|a, b| {
        a.hits
            .cmp(&b.hits)
            .then(a.touched.cmp(&b.touched))
            .then(a.key.cmp(&b.key))
    });
    let mut evicted = 0;
    // Reserve room for the audit marker before removing any optional material.
    if state.is_object() {
        state["context_window"] = json!({"policy":"LFU with oldest-access tie break","max_request_bytes":MAX_BYTES,"original_request_bytes":before,"evicted":0,"note":"Optional historical records may be omitted, never deleted from storage. Current questions and action parameters remain intact."});
    }
    while bytes(&state, questions) > MAX_BYTES && !entries.is_empty() {
        let entry = entries.remove(0);
        remove(&mut state, &entry);
        evicted += 1;
        for other in &mut entries {
            if other.path == entry.path && other.index > entry.index {
                other.index -= 1;
            }
        }
        tx.execute(
            "DELETE FROM jev_lfu WHERE purpose=? AND key=?",
            params![purpose, entry.key],
        )
        .map_err(|e| e.to_string())?;
    }
    if state.is_object() {
        state["context_window"]["evicted"] = json!(evicted);
    }
    if bytes(&state, questions) > MAX_BYTES {
        // Every observation, including packed system prompts, has a bounded decision view.
        // Full originals remain with the caller for downstream generation.
        for cap in [2048, 1024, 512, 256, 128, 64, 16] {
            project(&mut state, cap, 0);
            if bytes(&state, questions) <= MAX_BYTES {
                break;
            }
        }
        if bytes(&state, questions) > MAX_BYTES {
            let exact = state.get("exact_arguments").cloned();
            state = json!({"context_window":{"projected":true,"original_request_bytes":before},"observation":"Input too structurally large; only a bounded decision view is available. Do not authorize external actions or infer missing facts."});
            if let Some(args) = exact {
                state["exact_arguments"] = args;
            }
        }
        state["context_window"]["projected"] = json!(true);
    }
    if bytes(&state, questions) > MAX_BYTES {
        return Err("Jev 单个问题定义超过请求预算；需拆分问题定义。".into());
    }
    for entry in entries {
        tx.execute(
            "UPDATE jev_lfu SET touched=? WHERE purpose=? AND key=?",
            params![data::now(), purpose, entry.key],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute("DELETE FROM jev_lfu WHERE rowid NOT IN (SELECT rowid FROM jev_lfu ORDER BY touched DESC LIMIT 4096)",[]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(state)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lfu_keeps_frequent_history_and_exact_action_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.sqlite")).unwrap();
        let q = json!({"q":{"type":"choice","criteria":{"yes":"yes","no":"no"}}});
        let hot = json!({"id":"hot","content":"热".repeat(2000)});
        for _ in 0..3 {
            prepare(&db, json!({"retrieved_memories":[hot]}), &q, "test").unwrap();
        }
        let command = json!({"command":"echo exact > result.txt"});
        let result=prepare(&db,json!({"retrieved_memories":[hot,{"id":"cold","content":"冷".repeat(7000)}],"tool_args":command}),&q,"test").unwrap();
        assert!(bytes(&result, &q) <= MAX_BYTES);
        assert_eq!(result["tool_args"], command);
        assert_eq!(result["retrieved_memories"][0]["id"], "hot");
        assert_eq!(result["context_window"]["evicted"], 1);
    }
    #[test]
    fn oversized_packed_context_is_projected_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.sqlite")).unwrap();
        let original = json!({"conversation":[{"role":"system","content":"长上下文".repeat(12000)},{"role":"user","content":"你好"}],"profiles":{"notes":"资料".repeat(30000)}});
        let view = prepare(&db, original.clone(), &json!({}), "test").unwrap();
        assert!(bytes(&view, &json!({})) <= MAX_BYTES);
        assert_eq!(view["context_window"]["projected"], true);
        assert_eq!(view["conversation"][1]["content"], "你好");
        assert!(original["profiles"]["notes"].as_str().unwrap().len() > MAX_BYTES);
    }
}
