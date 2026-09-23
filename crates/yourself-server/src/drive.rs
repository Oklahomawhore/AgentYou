//! Stable values, durable goals, and contact energy derived from actual exchanges.
use crate::{
    data::{self, AppResult, Database},
    jev,
    service::App,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;
pub fn core() -> Value {
    json!({"values":["尊重用户的注意力、自主性和边界，不以持续打扰换取回应","诚实理解世界，区分事实、猜测和自身真实经历","保持好奇与独立判断，主动学习并产出有用的东西","有始有终，以真实反馈和可验证结果修正行动"],"purpose":"建立互相理解且有边界的关系，帮助用户和自己推进有意义的事情。情感目标可以是理解一种感受、消除误解或提供恰当陪伴；不能把迫使回应、制造依赖或聊天次数作为成功。","stability":"这是稳定的价值内核；具体偏好与方法可随经验变化，不能因为一条新闻或沉默就重写内核。"})
}
pub(crate) fn ensure_contact_energy(db: &rusqlite::Connection) -> AppResult<()> {
    let count:i64=db.query_row("SELECT COUNT(*) FROM messages WHERE role='assistant' AND mode='proactive' AND status='done' AND rowid>COALESCE((SELECT MAX(rowid) FROM messages WHERE role='user' AND status='done'),0)",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    if count >= 3 {
        Err("主动联系能量已耗尽，等待用户新消息恢复。".into())
    } else {
        Ok(())
    }
}
pub fn energy(db: &Database) -> AppResult<Value> {
    let count:i64=db.lock().query_row("SELECT COUNT(*) FROM messages WHERE role='assistant' AND mode='proactive' AND status='done' AND rowid>COALESCE((SELECT MAX(rowid) FROM messages WHERE role='user' AND status='done'),0)",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    Ok(
        json!({"capacity":3,"remaining":(3-count).max(0),"unanswered_proactive":count,"can_initiate":count<3,"rule":"主动消息每条消耗一格；连续三条未回应后不得继续主动发消息。新用户消息恢复满格；时间流逝不自动恢复。内部整理和做事不消耗联系能量，正常回复用户不消耗。"}),
    )
}
pub fn goal(db: &Database) -> AppResult<Value> {
    let payload: String = db
        .lock()
        .query_row("SELECT payload FROM agent_drive WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&payload).map_err(|e| e.to_string())
}
pub async fn refresh(app: &Arc<App>) -> AppResult<()> {
    if !app.router.config().uses_jev()
        || app
            .current(app.epoch.load(std::sync::atomic::Ordering::SeqCst))
            .is_err()
    {
        return Ok(());
    }
    let (event, last): (String, String) = {
        let db = app.db.lock();
        let event: String = db
            .query_row(
                "SELECT CAST(COALESCE(MAX(rowid),0) AS TEXT)||':'||COALESCE((SELECT MAX(rowid) FROM workspace_events),0) FROM messages",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let last: String = db
            .query_row(
                "SELECT reviewed_event FROM agent_drive WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        (format!("{}:{}", event, data::now() / 3_600_000), last)
    };
    if event == last {
        return Ok(());
    }
    let attempted: i64 = app
        .db
        .lock()
        .query_row("SELECT attempted_at FROM agent_drive WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if data::now() - attempted < 60_000 {
        return Ok(());
    }
    app.db
        .lock()
        .execute(
            "UPDATE agent_drive SET attempted_at=? WHERE id=1",
            [data::now()],
        )
        .map_err(|e| e.to_string())?;
    let dialogue_epoch = app.dialogue_epoch.load(std::sync::atomic::Ordering::SeqCst);
    let epoch = app.epoch.load(std::sync::atomic::Ordering::SeqCst);
    let (state, _) = app.loop_state()?;
    let state = crate::heartbeat::decision_context(state);
    let old = goal(&app.db)?;
    let response=app.router.complete(app.router.config().active_model(),vec![json!({"role":"system","content":"你在主循环中提出目标提案，不直接决定采用。依据精神内核、当前已知事实和已有目标，保持目标连续性，避免每轮换题。返回 JSON 对象，含 objective（具体目的）、kind（emotional 或 practical）、why、next_step（当前可执行的小步）、success_criteria（可验证结果）、evidence（已有实际证据，未完成则说未完成）。每字段不超过 300 字。禁止以消息数、逼用户回应或制造依赖为目标；用户不回不是完成证据。无需新目标时沿用原目标。"}),json!({"role":"user","content":state.to_string()})],"goal_proposal",Some(json!({"type":"json_object"})),None,false).await?;
    let proposal: Value = serde_json::from_str(&crate::openrouter::content(&response)?)
        .map_err(|_| "目标提案 JSON 无效")?;
    let mut clean = json!({});
    for field in [
        "objective",
        "kind",
        "why",
        "next_step",
        "success_criteria",
        "evidence",
    ] {
        let text = proposal[field]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("目标提案字段缺失")?;
        clean[field] = json!(data::short(text, 300));
    }
    let r=app.router.system_one(json!({"core":core(),"existing_goal":old,"proposal":clean,"actual_context":state}),json!({"goal":jev::choice("Decide the persistent goal lifecycle using actual evidence and stable values. Adopt a grounded proposal if no active goal or a justified revision exists. Keep an unfinished useful goal instead of inventing new conversation. Complete ONLY with observable success evidence, never merely because text was generated or user stayed silent. Abandon when inappropriate or declined. Emotional support must respect autonomy.",json!({"adopt":"Adopt this specific grounded proposal as active goal","keep":"Keep the existing goal unchanged","complete":"Existing active goal demonstrably achieved; record evidence","abandon":"Existing goal no longer appropriate; record reason"}))}),"jev_goal").await?;
    let action = jev::selected_logged(
        &app.db,
        &r["answers"]["goal"],
        &["adopt", "keep", "complete", "abandon"],
    )?;
    let mut next = old.clone();
    match action {
        "adopt" => {
            next = clean.clone();
            next["status"] = json!("active");
        }
        "complete" | "abandon" if old["status"] == "active" => {
            next["status"] = json!(if action == "complete" {
                "completed"
            } else {
                "abandoned"
            });
            next["result_evidence"] = clean["evidence"].clone();
        }
        _ => {}
    }
    let _guard = app.control.lock().await;
    app.current(epoch)?;
    if dialogue_epoch != app.dialogue_epoch.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("新用户消息到达，下一轮重新评估目标。".into());
    }
    let mut db = app.db.lock();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_drive SET payload=?,reviewed_event=? WHERE id=1",
        params![next.to_string(), event],
    )
    .map_err(|e| e.to_string())?;
    if next != old {
        tx.execute(
            "INSERT INTO goal_history VALUES(?,?,?,?)",
            params![data::id(), data::now(), action, next.to_string()],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}
