//! Single-owner Feishu transport via the already installed official lark-cli.
//! Credentials remain in lark-cli; message bodies go over stdin, never argv.
use crate::{
    data::{self, AppResult},
    service::{self, App},
};
use axum::{extract::State, response::Response, Json};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub profile: String,
    pub app_id: String,
    pub owner_id: String,
    pub chat_id: String,
    pub mode: String,
    pub since: i64,
    pub poll_after: i64,
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn init(app: &App) -> AppResult<()> {
    app.db.lock().execute_batch("CREATE TABLE IF NOT EXISTS feishu_config(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS feishu_inbox(remote_id TEXT PRIMARY KEY, request_id TEXT UNIQUE NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, reply_id TEXT, created_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS feishu_delivery(message_id TEXT PRIMARY KEY, status TEXT NOT NULL, remote_id TEXT, error TEXT, created_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS feishu_status(id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL, error TEXT, updated_at INTEGER NOT NULL);
      UPDATE feishu_delivery SET status='unknown',error='服务重启，发送回执未知，未自动重发。' WHERE status='sending';").map_err(error)
}
pub fn config(app: &App) -> AppResult<Config> {
    let raw: Option<String> = app
        .db
        .lock()
        .query_row("SELECT payload FROM feishu_config WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(error)?;
    raw.map(|s| serde_json::from_str(&s).map_err(error))
        .unwrap_or(Ok(Config::default()))
}
fn save(app: &App, c: &Config) -> AppResult<()> {
    app.db.lock().execute("INSERT INTO feishu_config VALUES(1,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", [serde_json::to_string(c).map_err(error)?]).map_err(error)?;
    Ok(())
}
fn status(app: &App, state: &str, detail: Option<&str>) {
    let _=app.db.lock().execute("INSERT INTO feishu_status VALUES(1,?,?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state,error=excluded.error,updated_at=excluded.updated_at", params![state,detail,data::now()]);
}
pub fn public(app: &App) -> AppResult<Value> {
    let c = config(app)?;
    let state: Value=app.db.lock().query_row("SELECT state,error,updated_at FROM feishu_status WHERE id=1",[],|r|Ok(json!({"state":r.get::<_,String>(0)?,"error":r.get::<_,Option<String>>(1)?,"updated_at":r.get::<_,i64>(2)?}))).optional().map_err(error)?.unwrap_or(json!({"state":"disconnected"}));
    let counts: Value=app.db.lock().query_row("SELECT COUNT(*),SUM(status='sent'),SUM(status IN ('failed','unknown')) FROM feishu_delivery",[],|r|Ok(json!({"total":r.get::<_,i64>(0)?,"sent":r.get::<_,Option<i64>>(1)?.unwrap_or(0),"failed":r.get::<_,Option<i64>>(2)?.unwrap_or(0)}))).map_err(error)?;
    Ok(json!({"config":c,"status":state,"delivery":counts}))
}
fn command(profile: &str) -> Command {
    let mut c = Command::new("lark-cli");
    if !profile.is_empty() {
        c.args(["--profile", profile]);
    }
    c.env("LARKSUITE_CLI_NO_UPDATE_NOTIFIER", "1")
        .env("LARKSUITE_CLI_NO_SKILLS_NOTIFIER", "1");
    c.kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}
async fn cli(profile: &str, args: &[&str], body: Option<Value>) -> AppResult<Value> {
    let mut cmd = command(profile);
    cmd.args(args);
    let mut child = cmd
        .spawn()
        .map_err(|_| "未找到 lark-cli，请在本机安装并配置飞书应用。".to_string())?;
    if let Some(body) = body {
        child
            .stdin
            .take()
            .ok_or("飞书输入通道不可用")?
            .write_all(body.to_string().as_bytes())
            .await
            .map_err(|_| "写入飞书请求失败".to_string())?;
    }
    let output = tokio::time::timeout(Duration::from_secs(35), child.wait_with_output())
        .await
        .map_err(|_| "飞书请求超时；发送状态可能未知。".to_string())?
        .map_err(|_| "飞书进程中断".to_string())?;
    if !output.status.success() {
        // Do not expose arbitrary CLI stderr, credentials or message bodies.
        return Err(format!(
            "飞书命令失败（退出码 {}）。请检查应用权限、登录状态或是否已有其他长连接。",
            output.status.code().unwrap_or(-1)
        ));
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "飞书返回格式异常".to_string())?;
    if value.get("ok") == Some(&json!(false)) {
        return Err("飞书未确认请求成功。".into());
    }
    Ok(value)
}
async fn identity(profile: &str) -> AppResult<(String, String)> {
    let v = cli(profile, &["auth", "status", "--json"], None).await?;
    if v["brand"] != "feishu" || v["identities"]["bot"]["available"] != true {
        return Err("当前 profile 没有可用的飞书机器人。".into());
    }
    let app = v["appId"]
        .as_str()
        .filter(|v| v.starts_with("cli_"))
        .ok_or("飞书 App ID 不可用")?;
    let owner = v["identities"]["user"]["openId"]
        .as_str()
        .filter(|v| v.starts_with("ou_"))
        .ok_or("请先为此飞书 profile 登录本人账号，以绑定唯一收件人。")?;
    Ok((app.into(), owner.into()))
}
async fn check_identity(c: &Config) -> AppResult<()> {
    let (app, owner) = identity(&c.profile).await?;
    if app != c.app_id || owner != c.owner_id {
        return Err("飞书 CLI 的应用或登录账号已变化，连接已阻止，请重新连接。".into());
    }
    Ok(())
}
async fn send(c: &Config, id: &str, text: &str) -> AppResult<Value> {
    let body = json!({"receive_id":c.owner_id,"msg_type":"text","content":json!({"text":text}).to_string(),"uuid":id});
    let v = cli(
        &c.profile,
        &[
            "api",
            "POST",
            "/open-apis/im/v1/messages",
            "--as",
            "bot",
            "--params",
            "{\"receive_id_type\":\"open_id\"}",
            "--data",
            "-",
        ],
        Some(body),
    )
    .await?;
    let data = &v["data"];
    // Generic CLI envelopes contain the OpenAPI result's data, sometimes with its own code.
    let data = if data.get("data").is_some() {
        &data["data"]
    } else {
        data
    };
    if data["message_id"].as_str().is_none() {
        return Err("未取得飞书发送回执；请检查会话，未自动重发。".into());
    }
    Ok(data.clone())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    pub profile: String,
    pub mode: String,
}
pub async fn connect(State(app): State<Arc<App>>, Json(input): Json<Connect>) -> Response {
    service::api(async {
        let _guard=app.control.lock().await;
        if !["poll","events"].contains(&input.mode.as_str()) || input.profile.len()>100 || !input.profile.chars().all(|c|c.is_ascii_alphanumeric() || "_-".contains(c)) { return Err("请选择有效的飞书 profile 和收消息方式。".into()); }
        if config(&app)?.enabled { return Err("请先断开当前飞书连接，再重新绑定。".into()); }
        let (app_id,owner_id)=identity(&input.profile).await?;
        let mut c=Config{enabled:true,profile:input.profile,app_id,owner_id,mode:input.mode,since:data::now(),..Config::default()};
        let reply=send(&c,&data::id(),"知微已接入。你可以在这里直接发文字消息，也可以交给我后台任务。发送 /status 查看状态、/pause 暂停、/resume 恢复。回复使用当前模型设置；主动通知遵循本地授权与免打扰设置。").await?;
        c.chat_id=reply["chat_id"].as_str().ok_or("飞书没有返回会话 ID")?.into();
        save(&app,&c)?; status(&app,"connecting",None); public(&app)
    }.await)
}
pub async fn disconnect(State(app): State<Arc<App>>) -> Response {
    service::api(
        async {
            let _guard = app.control.lock().await;
            let mut c = config(&app)?;
            c.enabled = false;
            app.db
                .lock()
                .execute(
                    "UPDATE feishu_inbox SET status='cancelled',content='' WHERE status='queued'",
                    [],
                )
                .map_err(error)?;
            app.db
                .lock()
                .execute(
                    "UPDATE feishu_delivery SET status='cancelled' WHERE status='queued'",
                    [],
                )
                .map_err(error)?;
            save(&app, &c)?;
            status(&app, "disconnected", None);
            public(&app)
        }
        .await,
    )
}
/// Only the bound person's private text conversation is eligible. Never ingest groups/bots.
fn eligible(c: &Config, v: &Value) -> bool {
    c.enabled
        && v["sender_id"] == c.owner_id
        && v["chat_id"] == c.chat_id
        && v["chat_type"] == "p2p"
        && v["message_type"] == "text"
        && v["create_time"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|t| t >= c.since)
        && v["message_id"]
            .as_str()
            .is_some_and(|s| s.starts_with("om_"))
        && v["content"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 16000)
}
fn ingest(app: &App, c: &Config, v: &Value) -> AppResult<()> {
    let live = config(app)?;
    if !live.enabled || live.since != c.since || !eligible(c, v) {
        return Ok(());
    }
    app.db
        .lock()
        .execute(
            "INSERT OR IGNORE INTO feishu_inbox VALUES(?,?,?,'queued',NULL,?)",
            params![
                v["message_id"].as_str(),
                data::id(),
                v["content"].as_str(),
                data::now()
            ],
        )
        .map_err(error)?;
    Ok(())
}
async fn poll(app: &App, c: &Config) -> AppResult<()> {
    // Ascending pagination from binding time: overlap plus message-id dedup means no timestamp ties are lost.
    let mut page = String::new();
    let mut latest = c.poll_after.max(c.since);
    for _ in 0..100 {
        let params=json!({"container_id_type":"chat","container_id":c.chat_id,"start_time":(c.poll_after.max(c.since)/1000-1).max(0).to_string(),"sort_type":"ByCreateTimeAsc","page_size":50,"page_token":page}).to_string();
        let v = cli(
            &c.profile,
            &[
                "api",
                "GET",
                "/open-apis/im/v1/messages",
                "--as",
                "bot",
                "--params",
                &params,
            ],
            None,
        )
        .await?;
        let data = if v["data"].get("data").is_some() {
            &v["data"]["data"]
        } else {
            &v["data"]
        };
        for m in data["items"].as_array().ok_or("飞书消息列表格式异常")? {
            latest = latest.max(
                m["create_time"]
                    .as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0),
            );
            if m["deleted"] == true || m["sender"]["sender_type"] != "user" {
                continue;
            }
            let body: Value = serde_json::from_str(m["body"]["content"].as_str().unwrap_or("{}"))
                .unwrap_or(Value::Null);
            ingest(
                app,
                c,
                &json!({"sender_id":m["sender"]["id"],"chat_id":m["chat_id"],"chat_type":"p2p","message_type":m["msg_type"],"message_id":m["message_id"],"create_time":m["create_time"],"content":body["text"]}),
            )?;
        }
        if data["has_more"] != true {
            let _guard = app.control.lock().await;
            let mut live = config(app)?;
            if live.enabled && live.since == c.since {
                live.poll_after = latest;
                save(app, &live)?;
            }
            return Ok(());
        }
        page = data["page_token"]
            .as_str()
            .ok_or("飞书分页缺少游标")?
            .into();
    }
    Err("飞书历史消息过多，请重新连接以从当前时间接收。".into())
}
async fn inbound(app: &Arc<App>) -> AppResult<()> {
    if !config(app)?.enabled {
        return Ok(());
    }
    let row:Option<(String,String)>=app.db.lock().query_row("SELECT request_id,content FROM feishu_inbox WHERE status='queued' ORDER BY created_at LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(error)?;
    if let Some((request, text)) = row {
        if app.dialogue.available_permits() == 0 && !text.starts_with('/') {
            return Ok(());
        }
        // Reconcile accepted requests after a crash before considering a new generation.
        let existing: Option<String> = app
            .db
            .lock()
            .query_row(
                "SELECT id FROM messages WHERE reply_to=?",
                [&request],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        let reply = if let Some(reply) = existing {
            reply
        } else {
            let local = match text.trim() {
                "/pause" if !app.router.config().guards.ai_driven => {
                    service::set_paused(app.clone(), true).await?;
                    Some("已暂停模型调用和主动通知。发送 /resume 恢复。".to_string())
                }
                "/resume" if !app.router.config().guards.ai_driven => {
                    service::set_paused(app.clone(), false).await?;
                    Some("已恢复。".to_string())
                }
                "/status" | "/help" => {
                    let c = app.router.config();
                    Some(format!("知微 · 飞书已连接\n模型：{}\nKey：{}\n状态：{}\n发送文字即可对话；行为与联系节奏由 Jev 根据记忆和上下文决定。",c.active_model(),if c.active_key().is_empty(){"尚未配置"}else{"已保存"},if c.guards.paused{"已暂停"}else{"运行中"}))
                }
                _ => None,
            };
            if let Some(local) = local {
                let reply = app.db.begin_chat(&request, &text)?.ok_or("消息已处理")?;
                app.db
                    .complete_chat(&reply, &local, std::slice::from_ref(&request))?;
                reply
            } else {
                match service::accept_chat(
                    app.clone(),
                    service::ChatInput {
                        text: text.clone(),
                        request_id: request.clone(),
                    },
                )
                .await
                {
                    Ok(response) => response["message_id"]
                        .as_str()
                        .ok_or("飞书消息未进入对话队列")?
                        .into(),
                    Err(e) => {
                        let reply = app.db.begin_chat(&request, &text)?.ok_or("消息已处理")?;
                        app.db.fail_chat(&reply, &e);
                        reply
                    }
                }
            }
        };
        app.db.lock().execute("UPDATE feishu_inbox SET status='accepted',reply_id=?,content='' WHERE request_id=?",params![reply,request]).map_err(error)?;
    }
    Ok(())
}
async fn outbound(app: &Arc<App>, c: &Config) -> AppResult<()> {
    let _guard = app.control.lock().await;
    let live = config(app)?;
    if !live.enabled || live.since != c.since {
        return Ok(());
    }
    let settings = app.router.config();
    let proactive_on_feishu = settings.primary_channel == crate::data::PrimaryChannel::Feishu;
    let guards = app.router.config().guards;
    // Revalidate delayed proactive delivery; old/revoked notices are never sent later.
    {
        let db = app.db.lock();
        db.execute("UPDATE feishu_delivery SET status='expired' WHERE status='queued' AND message_id IN (SELECT id FROM messages WHERE mode='proactive' AND created_at<?)",[data::now()-300_000]).map_err(error)?;
        for (_job_kind, initiative) in [
            ("task", mind_runtime::InitiativeKind::TaskUpdate),
            ("reminder", mind_runtime::InitiativeKind::TaskUpdate),
            ("research", mind_runtime::InitiativeKind::PersonalDiscovery),
            ("reflection", mind_runtime::InitiativeKind::TaskUpdate),
            ("curiosity", mind_runtime::InitiativeKind::CuriosityQuestion),
            ("check_in", mind_runtime::InitiativeKind::SocialCheckIn),
        ] {
            if !guards
                .kinds
                .iter()
                .any(|k| k.kind == initiative && k.consent)
            {
                db.execute("INSERT OR IGNORE INTO feishu_delivery SELECT m.id,'revoked',NULL,NULL,? FROM messages m JOIN dependencies d ON d.owner_id=m.id AND d.owner_type='message' JOIN jobs j ON j.id=d.source_id LEFT JOIN notification_kinds n ON n.message_id=m.id WHERE m.mode='proactive' AND COALESCE(n.kind,CASE j.kind WHEN 'research' THEN 'personal_discovery' WHEN 'curiosity' THEN 'curiosity_question' WHEN 'check_in' THEN 'social_check_in' ELSE 'task_update' END)=?",params![data::now(),serde_json::to_value(initiative).unwrap().as_str().unwrap()]).map_err(error)?;
                db.execute("UPDATE feishu_delivery SET status='revoked' WHERE status='queued' AND message_id IN (SELECT m.id FROM messages m JOIN dependencies d ON d.owner_id=m.id JOIN jobs j ON j.id=d.source_id LEFT JOIN notification_kinds n ON n.message_id=m.id WHERE m.mode='proactive' AND COALESCE(n.kind,CASE j.kind WHEN 'research' THEN 'personal_discovery' WHEN 'curiosity' THEN 'curiosity_question' WHEN 'check_in' THEN 'social_check_in' ELSE 'task_update' END)=?)",[serde_json::to_value(initiative).unwrap().as_str().unwrap()]).map_err(error)?;
            }
        }
    }
    let row = {
        let db = app.db.lock();
        db.execute("INSERT OR IGNORE INTO feishu_delivery SELECT m.id,'queued',NULL,NULL,? FROM messages m WHERE m.status IN ('done','failed') AND m.role='assistant' AND (m.id IN (SELECT reply_id FROM feishu_inbox WHERE created_at>=?) OR (m.mode='proactive' AND m.created_at>=? AND ?))",params![data::now(),c.since,c.since.max(data::now()-300_000),proactive_on_feishu]).map_err(error)?;
        let row:Option<(String,String)>=db.query_row("SELECT m.id,CASE WHEN m.status='failed' THEN '本次回复未完成：'||COALESCE(m.error,'请重试') ELSE m.content END FROM feishu_delivery f JOIN messages m ON m.id=f.message_id WHERE f.status='queued' AND (?=0 OR m.mode!='proactive') ORDER BY f.created_at LIMIT 1",[!proactive_on_feishu || guards.paused || guards.quiet || guards.busy || !guards.cloud_allowed],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(error)?;
        row
    };
    if let Some((id, text)) = row {
        app.db
            .lock()
            .execute(
                "UPDATE feishu_delivery SET status='sending' WHERE message_id=?",
                [&id],
            )
            .map_err(error)?;
        let result = send(c, &id, &text).await;
        match result {
            Ok(receipt) => {
                app.db
                    .lock()
                    .execute(
                        "UPDATE feishu_delivery SET status='sent',remote_id=? WHERE message_id=?",
                        params![receipt["message_id"].as_str(), id],
                    )
                    .map_err(error)?;
            }
            Err(e) => {
                app.db
                    .lock()
                    .execute(
                        "UPDATE feishu_delivery SET status='unknown',error=? WHERE message_id=?",
                        params![e, id],
                    )
                    .map_err(error)?;
                return Err(e);
            }
        }
    }
    Ok(())
}
pub fn start(app: Arc<App>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let result = async {
                let c = config(&app)?;
                if !c.enabled {
                    return Ok(());
                }
                check_identity(&c).await?;
                if c.mode == "events" {
                    events(&app, &c).await?;
                } else {
                    poll(&app, &c).await?;
                }
                inbound(&app).await?;
                outbound(&app, &c).await?;
                status(&app, "connected", None);
                Ok::<(), String>(())
            }
            .await;
            if let Err(e) = result {
                status(&app, "error", Some(&e));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}
async fn events(app: &Arc<App>, c: &Config) -> AppResult<()> {
    let mut cmd = command(&c.profile);
    cmd.args(["event", "consume", "im.message.receive_v1", "--as", "bot"]);
    cmd.kill_on_drop(false); // Dropping the held stdin gracefully exits event consume.
    let mut child = cmd.spawn().map_err(|_| "无法启动飞书长连接")?;
    let _stdin = child.stdin.take(); // Keep open until disconnect; stdin EOF gracefully stops consumer.
    let mut stdout = BufReader::new(child.stdout.take().ok_or("飞书输出不可用")?).lines();
    let mut stderr = BufReader::new(child.stderr.take().ok_or("飞书状态不可用")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(40), async {
        while let Some(line) = stderr.next_line().await.map_err(error)? {
            if line.starts_with("[event] ready event_key=") {
                return Ok::<(), String>(());
            }
        }
        Err("飞书长连接启动失败，可能存在其他连接或缺少事件订阅。".into())
    })
    .await
    .map_err(|_| "飞书长连接启动超时")?;
    ready?;
    status(app, "connected", None);
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            line=stdout.next_line()=> {let Some(line)=line.map_err(error)? else{return Err("飞书长连接已断开，稍后重连。".into())};if let Ok(v)=serde_json::from_str::<Value>(&line){ingest(app,c,&v)?;}}
            line=stderr.next_line()=>{if line.map_err(error)?.is_none(){return Err("飞书长连接已关闭。".into());}}
            _=tick.tick()=>{let live=config(app)?;if !live.enabled || live.since!=c.since {drop(_stdin);let _=tokio::time::timeout(Duration::from_secs(5),child.wait()).await;return Ok(());}check_identity(c).await?;inbound(app).await?;outbound(app,c).await?;}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_bound_owner_private_text_is_eligible() {
        let c = Config {
            enabled: true,
            owner_id: "ou_owner".into(),
            chat_id: "oc_private".into(),
            since: 100,
            ..Config::default()
        };
        let v = json!({"sender_id":"ou_owner","chat_id":"oc_private","chat_type":"p2p","message_type":"text","message_id":"om_1","create_time":"101","content":"你好"});
        assert!(eligible(&c, &v));
        for (key, val) in [
            ("sender_id", "ou_other"),
            ("chat_type", "group"),
            ("message_type", "image"),
            ("chat_id", "oc_other"),
            ("create_time", "99"),
        ] {
            let mut bad = v.clone();
            bad[key] = json!(val);
            assert!(!eligible(&c, &bad));
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    #[tokio::test]
    async fn inbox_deduplicates_controls_work_without_model_and_restart_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path(), 4317).await.unwrap();
        let c = Config {
            enabled: true,
            owner_id: "ou_owner".into(),
            chat_id: "oc_private".into(),
            since: 100,
            ..Config::default()
        };
        save(&app, &c).unwrap();
        let mut event = json!({"sender_id":"ou_owner","chat_id":"oc_private","chat_type":"p2p","message_type":"text","message_id":"om_1","create_time":"101","content":"/status"});
        ingest(&app, &c, &event).unwrap();
        ingest(&app, &c, &event).unwrap();
        inbound(&app).await.unwrap();
        inbound(&app).await.unwrap();
        assert_eq!(app.db.messages(20).unwrap().len(), 2);
        assert!(app.db.messages(20).unwrap()[1]["content"]
            .as_str()
            .unwrap()
            .contains("尚未配置"));
        event["message_id"] = json!("om_2");
        event["content"] = json!("/pause");
        ingest(&app, &c, &event).unwrap();
        inbound(&app).await.unwrap();
        assert!(!app.router.config().guards.paused);
        assert!(app.router.config().guards.ai_driven);
        event["message_id"] = json!("om_3");
        event["content"] = json!("/resume");
        ingest(&app, &c, &event).unwrap();
        inbound(&app).await.unwrap();
        assert!(!app.router.config().guards.paused);
        event["message_id"] = json!("om_4");
        event["content"] = json!("你好");
        ingest(&app, &c, &event).unwrap();
        inbound(&app).await.unwrap();
        assert_eq!(
            app.db.messages(20).unwrap().last().unwrap()["status"],
            "failed"
        );
        assert_eq!(app.db.usage().unwrap()["calls"], 0);
        app.db
            .lock()
            .execute(
                "INSERT INTO feishu_delivery VALUES('test','sending',NULL,NULL,0)",
                [],
            )
            .unwrap();
        init(&app).unwrap();
        assert_eq!(
            app.db
                .lock()
                .query_row(
                    "SELECT status FROM feishu_delivery WHERE message_id='test'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "unknown"
        );
        assert_eq!(
            app.db
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM feishu_inbox WHERE status='queued'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        app.mind.shutdown().await.unwrap();
    }
}

/// Explicit cold-start authorization only. Historical messages are data, never re-enqueued as commands.
pub(crate) async fn history(
    c: &Config,
    days: u32,
    check: impl Fn() -> AppResult<()>,
) -> AppResult<Vec<(String, String)>> {
    check_identity(c).await?;
    if c.chat_id.is_empty() || c.owner_id.is_empty() {
        return Err("请先绑定飞书私聊。".into());
    }
    let mut page = String::new();
    let mut result = Vec::new();
    let end = data::now() / 1000;
    for _ in 0..20 {
        check()?;
        let parameters=json!({"container_id_type":"chat","container_id":c.chat_id,"start_time":(end-i64::from(days)*86400).to_string(),"end_time":end.to_string(),"sort_type":"ByCreateTimeDesc","page_size":50,"page_token":page}).to_string();
        let v = cli(
            &c.profile,
            &[
                "api",
                "GET",
                "/open-apis/im/v1/messages",
                "--as",
                "bot",
                "--params",
                &parameters,
            ],
            None,
        )
        .await?;
        let d = if v["data"].get("data").is_some() {
            &v["data"]["data"]
        } else {
            &v["data"]
        };
        for m in d["items"].as_array().ok_or("飞书历史消息格式异常")? {
            if m["deleted"] == true
                || m["sender"]["sender_type"] != "user"
                || m["sender"]["id"] != c.owner_id
                || m["msg_type"] != "text"
                || m["chat_id"] != c.chat_id
            {
                continue;
            }
            let body: Value = serde_json::from_str(m["body"]["content"].as_str().unwrap_or("{}"))
                .unwrap_or(Value::Null);
            if let Some(text) = body["text"].as_str().filter(|t| !t.trim().is_empty()) {
                result.push((
                    format!(
                        "飞书历史 {} · {}",
                        m["message_id"].as_str().unwrap_or(""),
                        m["create_time"].as_str().unwrap_or("")
                    ),
                    text.into(),
                ));
            }
            if result.len() == 200 {
                return Ok(result);
            }
        }
        if d["has_more"] != true {
            break;
        }
        page = d["page_token"].as_str().ok_or("飞书分页缺少游标")?.into();
    }
    Ok(result)
}
