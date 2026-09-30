//! Persistent autonomous wakeups; Jev chooses the action and the tool.
use crate::{
    data::{self, AppResult},
    jev,
    service::{self, App},
};
use axum::{extract::State, response::Response, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub public_topics: Vec<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_minutes: 1,
            public_topics: vec![],
        }
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn init(app: &App) -> AppResult<()> {
    crate::decision_tree::init(app)?;
    crate::execution::init(app)?;
    app.db.lock().execute_batch("CREATE TABLE IF NOT EXISTS agent_loop_state(id INTEGER PRIMARY KEY CHECK(id=1),phase TEXT NOT NULL,updated_at INTEGER NOT NULL,last_error TEXT);
        INSERT OR IGNORE INTO agent_loop_state VALUES(1,'idle',0,NULL);
        CREATE TABLE IF NOT EXISTS heartbeat_config(id INTEGER PRIMARY KEY CHECK(id=1),payload TEXT NOT NULL,next_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS heartbeat_runs(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,status TEXT NOT NULL,action TEXT NOT NULL DEFAULT '',tool TEXT NOT NULL DEFAULT '',job_id TEXT,error TEXT);
        UPDATE heartbeat_runs SET status='interrupted',error='服务重启，下个周期继续判断。' WHERE status='running';").map_err(err)?;
    app.db
        .lock()
        .execute(
            "INSERT OR IGNORE INTO heartbeat_config VALUES(1,?,?)",
            params![
                serde_json::to_string(&Config::default()).map_err(err)?,
                data::now() + 60_000
            ],
        )
        .map_err(err)?;
    Ok(())
}
pub fn config(app: &App) -> AppResult<Config> {
    let raw: String = app
        .db
        .lock()
        .query_row("SELECT payload FROM heartbeat_config WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(err)?;
    serde_json::from_str(&raw).map_err(err)
}
pub fn public(app: &App) -> AppResult<Value> {
    let c = config(app)?;
    let preferences = crate::decision_tree::public(app)?;
    let db = app.db.lock();
    let next: i64 = db
        .query_row("SELECT next_at FROM heartbeat_config WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(err)?;
    let mut q=db.prepare("SELECT r.id,r.created_at,r.status,r.action,r.tool,r.job_id,r.error,COALESCE(j.status,g.status),COALESCE(p.intention,g.objective) FROM heartbeat_runs r LEFT JOIN jobs j ON j.id=r.job_id LEFT JOIN execution_goals g ON g.id=r.job_id LEFT JOIN decision_paths p ON p.run_id=r.id ORDER BY r.created_at DESC LIMIT 20").map_err(err)?;
    let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"created_at":r.get::<_,i64>(1)?,"status":r.get::<_,String>(2)?,"action":r.get::<_,String>(3)?,"tool":r.get::<_,String>(4)?,"job_id":r.get::<_,Option<String>>(5)?,"error":r.get::<_,Option<String>>(6)?,"job_status":r.get::<_,Option<String>>(7)?,"intention":r.get::<_,Option<String>>(8)?}))).map_err(err)?;
    let phase:Value=db.query_row("SELECT phase,updated_at,last_error FROM agent_loop_state WHERE id=1",[],|r|Ok(json!({"phase":r.get::<_,String>(0)?,"updated_at":r.get::<_,i64>(1)?,"last_error":r.get::<_,Option<String>>(2)?}))).map_err(err)?;
    Ok(
        json!({"branch_tendencies":preferences,"config":c,"next_at":next,"loop":phase,"runs":rows.collect::<Result<Vec<_>,_>>().map_err(err)?}),
    )
}
pub async fn get(State(app): State<Arc<App>>) -> Response {
    service::api(public(&app))
}
pub async fn save(State(app): State<Arc<App>>, Json(mut c): Json<Config>) -> Response {
    service::api(async{
    if !(1..=1440).contains(&c.interval_minutes)||c.public_topics.len()>10||c.public_topics.iter().any(|s|s.trim().is_empty()||s.chars().count()>100){return Err("循环间隔为 1–1440 分钟；公开检索主题最多 10 个，每个 100 字。".into());}
    c.public_topics=c.public_topics.into_iter().map(|s|s.trim().into()).collect();
    let _guard=app.control.lock().await;
    app.db.lock().execute("UPDATE heartbeat_config SET payload=?,next_at=? WHERE id=1",params![serde_json::to_string(&c).map_err(err)?,data::now()+i64::from(c.interval_minutes)*60000]).map_err(err)?;
    // Cancel pending autonomous work on any scope change; newly allowed work must be replanned.
    app.db.lock().execute("UPDATE jobs SET status='cancelled',error='自主循环设置已变化，等待重新判断。' WHERE status IN ('queued','running') AND id IN (SELECT job_id FROM heartbeat_runs WHERE job_id IS NOT NULL)",[]).map_err(err)?;
    app.db.lock().execute("UPDATE execution_goals SET status='blocked',error='自主循环设置已变更，请检查后继续' WHERE autonomous=1 AND status IN ('queued','running')",[]).map_err(err)?;
    app.epoch.fetch_add(1,Ordering::SeqCst);Ok(json!({"ok":true}))
}.await)
}
pub async fn wake(State(app): State<Arc<App>>) -> Response {
    service::api(
        async {
            let _guard = app.control.lock().await;
            if !config(&app)?.enabled {
                return Err("请先开启自主循环。".into());
            }
            app.db
                .lock()
                .execute(
                    "UPDATE heartbeat_config SET next_at=? WHERE id=1",
                    [data::now()],
                )
                .map_err(err)?;
            Ok(json!({"ok":true}))
        }
        .await,
    )
}
pub(crate) async fn tick(app: &Arc<App>, now: i64) -> AppResult<()> {
    let c = config(app)?;
    let s = app.router.config();
    if !c.enabled
        || !s.uses_jev()
        || s.active_key().is_empty()
        || s.guards.paused
        || !s.guards.cloud_allowed
        || app.dialogue.available_permits() == 0
    {
        return Ok(());
    }
    if s.daily_call_limit != 0
        && app.db.usage()?["calls"].as_u64().unwrap_or(u64::MAX) >= u64::from(s.daily_call_limit)
    {
        return Ok(());
    }
    let epoch = app.epoch.load(Ordering::SeqCst);
    let dialogue_epoch = app.dialogue_epoch.load(Ordering::SeqCst);
    let id = data::id();
    {
        let _guard = app.control.lock().await;
        app.current(epoch)?;
        let mut db = app.db.lock();
        let tx = db.transaction().map_err(err)?;
        let due: i64 = tx
            .query_row("SELECT next_at FROM heartbeat_config WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        let occupied:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM heartbeat_runs WHERE status='running') OR EXISTS(SELECT 1 FROM jobs WHERE status='running' OR (status='queued' AND due_at<=?)) OR EXISTS(SELECT 1 FROM execution_goals WHERE status IN ('queued','running'))",[now],|r|r.get(0)).map_err(err)?;
        if now < due || occupied {
            return Ok(());
        }
        tx.execute(
            "UPDATE heartbeat_config SET next_at=? WHERE id=1",
            [now + i64::from(c.interval_minutes) * 60000],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO heartbeat_runs(id,created_at,status) VALUES(?,?,'running')",
            params![id, now],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
    }
    let result = decide(app, &id, &c, epoch, dialogue_epoch).await;
    if let Err(e) = &result {
        app.db
            .lock()
            .execute(
                "UPDATE heartbeat_runs SET status='failed',error=? WHERE id=?",
                params![e, id],
            )
            .map_err(err)?;
    }
    result
}
async fn decide(
    app: &Arc<App>,
    id: &str,
    c: &Config,
    epoch: u64,
    dialogue_epoch: u64,
) -> AppResult<()> {
    let (mut context, refs) = app.loop_state()?;
    context["environment"] = crate::environment::snapshot().await;
    if app.router.config().guards.ai_driven {
        let guard = crate::execution::RunContext {
            epoch,
            dialogue_epoch,
            autonomous: true,
        };
        return crate::execution::autonomous(app, id, context, refs, &guard).await;
    }
    let (action, intention, selected_context) =
        crate::decision_tree::decide(app, id, &context, &c.public_topics, epoch, dialogue_epoch)
            .await?;
    let context = if action == "no_action" {
        json!({})
    } else {
        selected_context
    };
    let mut tool = String::new();
    let mut objective = String::new();
    let mut kind = "";
    if action == "message" {
        kind = "autonomous_message";
        objective = intention.clone();
    }
    if action == "continue_interest" || action == "explore" {
        let mut choices = json!({"workspace_tools":"Use one of the tools explicitly listed in available_tools to advance an actual context-grounded objective. Workspace Read/Write/Bash are available; do not assume a browser is enabled.","learn_skill":"Extract or improve a reusable skill from actual successful tool results or explicit user guidance. Use SkillSave with source IDs; do not invent experience.","review_memory":"Review actual conversation, feedback and saved memories; clarify tentative self-understanding without inventing experiences."});
        for (i, topic) in c.public_topics.iter().enumerate() {
            choices[format!("topic_{i}")]=json!(format!("Use the external Wikipedia search/read tool on exactly this authorized public topic: {topic}"));
        }
        let r=app.router.system_one(json!(format!("本轮用意：{intention}\n{}",crate::decision_tree::context(&context,&["now_ms","purpose","recent_work","recent_tool_results_untrusted","recent_interactions","long_term_memory","available_skills","world_context"]))),json!({"tool":jev::choice("Choose ONE available self-organization tool useful now. Return the key of the tool; arguments are fixed by its description. Do not repeat a completed lookup without a reason.",choices.clone())}),"jev_heartbeat_tool").await?;
        let keys: Vec<&str> = choices
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        app.current(epoch)?;
        if app.dialogue_epoch.load(Ordering::SeqCst) != dialogue_epoch || !config(app)?.enabled {
            return Err("新对话或循环设置已改变，本轮工作停止。".into());
        }
        tool = jev::selected_logged(&app.db, &r["answers"]["tool"], &keys)?.into();
        if tool == "workspace_tools" || tool == "learn_skill" {
            let work_intention=format!("本轮工作用意：{intention}。根据实际上下文选择有用的工作空间操作；没有合适操作则不调用。");
            let output=crate::workspace::step(app,context.clone(),if tool == "learn_skill" { "从实际成功工具结果或用户明确指导中总结可复用方法，使用 SkillSave 保存步骤、适用条件、验证方式和真实 source_ids；先检查现有技能以避免重复，无依据则不保存。" } else { &work_intention }).await?;
            app.db
                .lock()
                .execute(
                    "UPDATE heartbeat_runs SET status='done',action=?,tool=? WHERE id=?",
                    params![action, tool, id],
                )
                .map_err(err)?;
            let _ = output;
            return Ok(());
        }
        if tool == "review_memory" {
            kind = "self_review";
            objective="回顾实际对话、反馈与记忆，整理自身经历、已有偏好、矛盾与待观察项。区分真实行为和生成草稿；没有新依据则明确保持不变，不强行改变人格。".into();
        } else {
            let i: usize = tool
                .strip_prefix("topic_")
                .ok_or("工具不存在")?
                .parse()
                .map_err(|_| "工具参数错误")?;
            objective = c.public_topics.get(i).ok_or("公开主题不存在")?.clone();
            kind = "knowledge_search";
        }
    }
    if action == "message" && app.router.config().guards.ai_driven {
        let references = refs.clone();
        let mut messages = vec![
            json!({"role":"system","content":format!("本轮选定用意：{intention}。以下为相关观察：{}",crate::decision_tree::context(&context,&["now_ms","environment","persona","purpose","long_term_memory","recent_interactions","recent_work","contact_history","world_context"]))}),
        ];
        messages.push(json!({"role":"system","content":"Jev 已决定主动联系用户。结合 purpose 的长期、中期、短期自然语言，写一条符合此刻意图的简短消息；可以做事、理解、陪伴或探索，不必追求任务结果，不索取回应，不连续重复追问，不在未获得证据时宣布完成。只有最近实际交互中出现的消息才算说过；不要把后台草稿当作已发送的问题，不责怪用户没有回答，不推测其隐藏动机。"}));
        let settings = app.router.config();
        let response = app
            .router
            .complete(
                settings.active_model(),
                messages,
                "autonomous_message",
                None,
                None,
                false,
            )
            .await?;
        let text = crate::openrouter::content(&response)?;
        let _guard = app.control.lock().await;
        app.current(epoch)?;
        if !config(app)?.enabled {
            return Err("自主循环已关闭。".into());
        }
        if app.dialogue_epoch.load(Ordering::SeqCst) != dialogue_epoch {
            return Err("用户已发来新消息，取消过时主动消息，下一轮重新判断。".into());
        }
        app.db.deliver(&format!("heartbeat-{id}"), &text, id)?;
        // Preserve source dependencies for memory erasure without turning this into a user task.
        let db = app.db.lock();
        for source in references {
            db.execute("INSERT OR IGNORE INTO dependencies SELECT 'message',message_id,? FROM outbox WHERE event_id=?",params![source,format!("heartbeat-{id}")]).map_err(err)?;
        }
        db.execute("UPDATE heartbeat_runs SET status='done',action='message',tool='',job_id=NULL WHERE id=?",[id]).map_err(err)?;
        return Ok(());
    }
    if (action == "continue_interest" || action == "explore")
        && app.router.config().guards.ai_driven
    {
        let output = if kind == "knowledge_search" {
            encyclopedia(&objective).await?
        } else {
            let mut messages = vec![
                json!({"role":"system","content":format!("本轮选定工作用意：{intention}。观察：{}",crate::decision_tree::context(&context,&["now_ms","purpose","recent_work","recent_tool_results_untrusted","recent_interactions","long_term_memory","world_context"]))}),
            ];
            messages.push(json!({"role":"system","content":"整理真实交互中的用户理解、自身偏好、尚未解决的问题。草稿不是亲身经历，不编造已完成动作。输出简短、有依据的整理记录。"}));
            let config = app.router.config();
            crate::openrouter::content(
                &app.router
                    .complete(
                        config.active_model(),
                        messages,
                        "self_organization",
                        None,
                        None,
                        false,
                    )
                    .await?,
            )?
        };
        let items = app
            .router
            .classify_memory(
                &output,
                "internal_organization_not_verified_fact",
                json!(app.db.relevant_memories(&objective, 20)?),
            )
            .await?;
        let changes = app
            .router
            .adapt_profiles(
                "",
                json!({"actual_context":context,"internal_summary":output}),
            )
            .await?;
        let _guard = app.control.lock().await;
        app.current(epoch)?;
        app.db
            .store_classified(id, "主循环内部整理", &items, &refs)?;
        app.db.commit_adaptation(id, &changes)?;
        app.db
            .lock()
            .execute(
                "INSERT INTO workspace_events VALUES(?,?,?)",
                params![
                    data::id(),
                    data::now(),
                    json!({"tool":tool,"result":output,"source":"internal_loop_not_user_dialogue"})
                        .to_string()
                ],
            )
            .map_err(err)?;
        app.db
            .lock()
            .execute(
                "UPDATE heartbeat_runs SET status='done',action=?,tool=?,job_id=NULL WHERE id=?",
                params![action, tool, id],
            )
            .map_err(err)?;
        return Ok(());
    }
    let _guard = app.control.lock().await;
    app.current(epoch)?;
    if app.dialogue_epoch.load(Ordering::SeqCst) != dialogue_epoch
        || app.dialogue.available_permits() == 0
    {
        return Err("新对话已开始，本次自主判断作废。".into());
    }
    let current = config(app)?;
    if !current.enabled {
        return Err("自主循环已关闭。".into());
    }
    let job = if kind.is_empty() {
        None
    } else {
        let id = app.db.create_job(kind, &objective, data::now())?;
        for source in refs {
            app.db
                .lock()
                .execute(
                    "INSERT OR IGNORE INTO dependencies VALUES('job',?,?)",
                    params![id, source],
                )
                .map_err(err)?;
        }
        Some(id)
    };
    app.db
        .lock()
        .execute(
            "UPDATE heartbeat_runs SET status='done',action=?,tool=?,job_id=? WHERE id=?",
            params![action, tool, job, id],
        )
        .map_err(err)?;
    Ok(())
}
fn phase(app: &App, name: &str, error: Option<&str>) {
    let _ = app.db.lock().execute(
        "UPDATE agent_loop_state SET phase=?,updated_at=?,last_error=? WHERE id=1",
        params![name, data::now(), error],
    );
}
/// The sole background reasoning loop. Channel I/O and interactive dialogue remain independent.
pub fn start(app: Arc<App>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            phase(&app, "attention", None);
            let _ = crate::attention::advance(&app.db, data::now());
            phase(&app, "news", None);
            let _ = crate::news::advance(&app).await;
            phase(&app, "hot_topics", None);
            let _ = crate::hot_topics::advance(&app).await;
            phase(&app, "github_weekly", None);
            let _ = crate::github_weekly::advance(&app).await;
            phase(&app, "import", None);
            let import = crate::bootstrap::advance(&app).await;
            phase(
                &app,
                "continuations",
                import.as_ref().err().map(String::as_str),
            );
            let goals = crate::execution::advance(&app).await;
            phase(
                &app,
                "goal_execution",
                goals.as_ref().err().map(String::as_str),
            );
            let work = crate::service::advance_work(&app).await;
            phase(&app, "decision", work.as_ref().err().map(String::as_str));
            if app.dialogue.available_permits() > 0 {
                if let Err(error) = crate::drive::refresh(&app).await {
                    phase(&app, "goal_error", Some(&error));
                }
            }
            let decision = tick(&app, data::now()).await;
            phase(&app, "idle", decision.as_ref().err().map(String::as_str));
            // Poll durable continuations; Jev wakeup cadence remains next_at (one minute).
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

/// A bounded external tool. Only explicit public topics are sent; no chat or memory.
pub async fn encyclopedia(topic: &str) -> AppResult<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("YourSelf/0.2 (personal knowledge research)")
        .build()
        .map_err(err)?;
    let mut r = client
        .get("https://zh.wikipedia.org/w/api.php")
        .query(&[
            ("action", "query"),
            ("format", "json"),
            ("formatversion", "2"),
            ("generator", "search"),
            ("gsrsearch", topic),
            ("gsrlimit", "3"),
            ("prop", "extracts"),
            ("exintro", "1"),
            ("explaintext", "1"),
            ("exchars", "1800"),
        ])
        .send()
        .await
        .map_err(|_| "公开百科检索连接失败或超时。")?;
    if !r.status().is_success() {
        return Err(format!(
            "公开百科检索失败（HTTP {}）。",
            r.status().as_u16()
        ));
    }
    let mut bytes = vec![];
    while let Some(chunk) = r.chunk().await.map_err(|_| "检索响应中断")? {
        if bytes.len() + chunk.len() > 128_000 {
            return Err("检索响应超过大小限制。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| "公开百科响应格式错误")?;
    parse_encyclopedia(&v)
}
pub(crate) fn parse_encyclopedia(v: &Value) -> AppResult<String> {
    if v.get("error").is_some() {
        return Err("公开百科接口返回错误。".into());
    }
    let pages = v["query"]["pages"].as_array();
    let mut result = vec![];
    for p in pages.into_iter().flatten().take(3) {
        if let (Some(id), Some(title), Some(extract)) = (
            p["pageid"].as_u64(),
            p["title"].as_str(),
            p["extract"].as_str(),
        ) {
            result.push(format!(
                "来源：{title}\nhttps://zh.wikipedia.org/?curid={id}\n{}",
                data::short(extract, 1800)
            ));
        }
    }
    if result.is_empty() {
        return Err("公开百科未找到可读取的内容，本轮未新增知识。".into());
    }
    Ok(result.join("\n\n"))
}

// Build a decision view; durable records and the independent dialogue context stay intact.
pub(crate) fn decision_context(mut context: Value) -> Value {
    fn excerpt(value: &mut Value, limit: usize, chars: usize, newest_last: bool) {
        if let Some(items) = value.as_array_mut() {
            if newest_last && items.len() > limit {
                items.drain(..items.len() - limit);
            } else {
                items.truncate(limit);
            }
            for item in items {
                if let Some(text) = item.as_str() {
                    *item = json!(data::short(text, chars));
                } else {
                    for key in ["content", "output", "objective"] {
                        if let Some(text) = item[key].as_str() {
                            let truncated = text.chars().count() > chars;
                            item[key] = json!(data::short(text, chars));
                            if truncated {
                                item["excerpt"] = json!(true);
                            }
                        }
                    }
                }
            }
        }
    }
    excerpt(
        &mut context["long_term_memory"]["retrieved_memories"],
        6,
        350,
        false,
    );
    excerpt(&mut context["recent_interactions"], 8, 700, true);
    excerpt(&mut context["recent_work"], 2, 700, false);
    excerpt(
        &mut context["recent_tool_results_untrusted"],
        2,
        1000,
        false,
    );
    context["context_view"] = json!("Decision excerpts, not complete records. Latest interactions are retained in chronological order; memories are relevance-ranked. Omitted text is unknown, not evidence of absence. Full records remain stored locally.");
    context
}
