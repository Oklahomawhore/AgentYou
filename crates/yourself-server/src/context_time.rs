//! Host-authored time labels; recording time is never evidence of event occurrence.
use crate::data::{AppResult, Database};
use serde_json::{json, Value};
pub fn label(db: &Database, stamp: Option<i64>, now: i64, kind: &str) -> AppResult<String> {
    let current: String = db
        .lock()
        .query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%S',?/1000,'unixepoch','+8 hours')",
            [now],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let Some(stamp) = stamp else {
        return Ok(format!("[上下文时间：{kind}未知；读取时间 {current}+08:00；时区 Asia/Shanghai；不得把读取时间当作事件发生时间]"));
    };
    let recorded: String = db
        .lock()
        .query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%S',?/1000,'unixepoch','+8 hours')",
            [stamp],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "[上下文时间：{kind} {recorded}+08:00；距本次组装 {} 秒；时区 Asia/Shanghai]",
        now.saturating_sub(stamp) / 1000
    ))
}
pub fn annotate(db: &Database, value: &mut Value, now: i64) -> AppResult<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                annotate(db, item, now)?;
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                annotate(db, child, now)?;
            }
            let timestamp = map.get("created_at").and_then(Value::as_i64);
            let updated = map.get("updated_at").and_then(Value::as_i64);
            if timestamp.is_some() || updated.is_some() {
                map.insert("record_time".into(),json!({"created":label(db,timestamp,now,"创建／记录时间")?,"updated":label(db,updated,now,"更新时间")?,"event_time":"除非正文明确说明，否则事件实际发生时间未知"}));
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn prepare_messages(
    db: &Database,
    mut messages: Vec<Value>,
    now: i64,
) -> AppResult<Vec<Value>> {
    let clock = label(db, Some(now), now, "当前请求时间")?;
    let instruction="依据明确时间计算间隔，不根据消息条数、语气或叙述长度推测过了几天。记录时间不等于事件发生时间；没有新记录不证明这段时间什么都没发生或文件没被改动。未知时间明确承认未知。";
    for (i, message) in messages.iter_mut().enumerate() {
        if message["role"] == "tool" || message["role"] == "assistant" {
            continue;
        }
        if let Some(text) = message["content"].as_str() {
            let mut content = text.to_owned();
            if !content.starts_with("[上下文时间：") {
                content = format!("{}\n{content}", label(db, None, now, "来源时间")?);
            }
            if i == 0 && message["role"] == "system" {
                content = format!("{clock}\n{instruction}\n{content}");
            }
            message["content"] = json!(content);
        }
    }
    Ok(messages)
}
