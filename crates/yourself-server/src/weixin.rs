//! Personal WeChat iLink transport with self-contained QR login.
use crate::{
    data::{self, AppResult, PrimaryChannel},
    service::{self, App},
};
use axum::{extract::State, response::Response};
use base64::Engine;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
const BASE: &str = "https://ilinkai.weixin.qq.com";
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    enabled: bool,
    token: String,
    base_url: String,
    owner: String,
    context: String,
    cursor: String,
    since: i64,
    generation: String,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn init(app: &App) -> AppResult<()> {
    app.db.lock().execute_batch("CREATE TABLE IF NOT EXISTS weixin_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS weixin_inbox(id TEXT PRIMARY KEY,request TEXT NOT NULL,content TEXT NOT NULL,reply TEXT,generation TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS weixin_delivery(id TEXT PRIMARY KEY,status TEXT NOT NULL,error TEXT);
    UPDATE weixin_delivery SET status='unknown',error='发送时服务重启，请核对微信；未自动重发。' WHERE status='sending';").map_err(err)
}
fn get(app: &App, key: &str) -> AppResult<Value> {
    let raw: Option<String> = app
        .db
        .lock()
        .query_row("SELECT value FROM weixin_state WHERE key=?", [key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    raw.map(|s| serde_json::from_str(&s).map_err(err))
        .unwrap_or(Ok(Value::Null))
}
fn put(app: &App, key: &str, value: Value) -> AppResult<()> {
    app.db.lock().execute("INSERT INTO weixin_state VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()]).map_err(err)?;
    Ok(())
}
fn config(app: &App) -> AppResult<Config> {
    let v = get(app, "config")?;
    if v.is_null() {
        Ok(Config::default())
    } else {
        serde_json::from_value(v).map_err(err)
    }
}
fn save(app: &App, c: &Config) -> AppResult<()> {
    put(app, "config", serde_json::to_value(c).map_err(err)?)
}
fn safe_base(s: &str) -> AppResult<String> {
    let u = reqwest::Url::parse(s).map_err(|_| "微信服务地址无效")?;
    if u.scheme() != "https"
        || !u.host_str().is_some_and(|h| h.ends_with(".weixin.qq.com"))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err("仅允许微信官方 HTTPS 服务地址".into());
    }
    Ok(s.trim_end_matches('/').into())
}
fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(40))
        .build()
        .map_err(|_| "微信客户端初始化失败".into())
}
async fn request(
    base: &str,
    endpoint: &str,
    token: Option<&str>,
    body: Option<Value>,
    query: &[(&str, &str)],
) -> AppResult<Value> {
    let url = format!("{}/ilink/bot/{endpoint}", safe_base(base)?);
    let client = client()?;
    let mut req = if let Some(mut body) = body {
        body["base_info"] = json!({"channel_version":"2.2.0"});
        client
            .post(url)
            .json(&body)
            .header("AuthorizationType", "ilink_bot_token")
            .header(
                "X-WECHAT-UIN",
                base64::engine::general_purpose::STANDARD
                    .encode((uuid::Uuid::new_v4().as_u128() as u32).to_string()),
            )
    } else {
        client.get(url).query(query)
    };
    req = req
        .header("iLink-App-Id", "bot")
        .header("iLink-App-ClientVersion", "131584");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req
        .send()
        .await
        .map_err(|_| "微信请求失败或超时".to_string())?;
    if !response.status().is_success() {
        return Err(format!("微信服务 HTTP {}", response.status().as_u16()));
    }
    let v: Value = response.json().await.map_err(|_| "微信返回格式异常")?;
    for key in ["ret", "errcode"] {
        if v[key].as_i64().is_some_and(|n| n != 0) {
            return Err(format!("微信接口错误 {}（会话失效时请重新扫码）", v[key]));
        }
    }
    Ok(v)
}
pub fn public(app: &App) -> AppResult<Value> {
    let c = config(app)?;
    let failed: i64 = app
        .db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM weixin_delivery WHERE status='unknown'",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    Ok(
        json!({"delivery_uncertain":failed,"enabled":c.enabled,"has_credentials":!c.token.is_empty(),"can_send":!c.context.is_empty(),"status":get(app,"status")?,"login_status":get(app,"login_status")?}),
    )
}
fn report(app: &App, result: &AppResult<()>) {
    let _ = put(
        app,
        "status",
        match result {
            Ok(_) => json!({"state":"connected"}),
            Err(e) => json!({"state":"error","error":e}),
        },
    );
}
pub async fn disconnect(State(app): State<Arc<App>>) -> Response {
    service::api(
        async {
            let _guard = app.control.lock().await;
            save(&app, &Config::default())?;
            put(&app, "login", Value::Null)?;
            put(&app, "login_status", Value::Null)?;
            put(&app, "status", json!({"state":"disconnected"}))?;
            public(&app)
        }
        .await,
    )
}
async fn qr_image(text: &str) -> AppResult<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|_| "二维码生成失败")?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}
pub async fn login(State(app): State<Arc<App>>) -> Response {
    service::api(
        async {
            let _guard = app.control.lock().await;
            if config(&app)?.enabled {
                return Err("请先断开现有微信连接".into());
            }
            let v = request(BASE, "get_bot_qrcode", None, None, &[("bot_type", "3")]).await?;
            let code = v["qrcode"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("微信未返回二维码")?;
            let image = qr_image(
                v["qrcode_img_content"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(code),
            )
            .await?;
            put(
                &app,
                "login",
                json!({"code":code,"base":BASE,"expires":data::now()+240_000}),
            )?;
            put(&app, "login_status", json!("wait"))?;
            Ok(json!({"image":image}))
        }
        .await,
    )
}
async fn poll_login(app: &Arc<App>) -> AppResult<()> {
    let pending = get(app, "login")?;
    let Some(code) = pending["code"].as_str() else {
        return Ok(());
    };
    if pending["expires"].as_i64().unwrap_or(0) < data::now() {
        put(app, "login", Value::Null)?;
        put(app, "login_status", json!("expired"))?;
        return Ok(());
    }
    let v = request(
        pending["base"].as_str().unwrap_or(BASE),
        "get_qrcode_status",
        None,
        None,
        &[("qrcode", code)],
    )
    .await?;
    let _guard = app.control.lock().await;
    if get(app, "login")? != pending {
        return Ok(());
    }
    let state = v["status"].as_str().unwrap_or("wait");
    put(app, "login_status", json!(state))?;
    if state == "scaned_but_redirect" {
        let base = safe_base(&format!(
            "https://{}",
            v["redirect_host"].as_str().ok_or("微信重定向缺少地址")?
        ))?;
        let mut next = pending;
        next["base"] = json!(base);
        put(app, "login", next)?;
    } else if state == "expired" {
        put(app, "login", Value::Null)?;
    } else if state == "confirmed" {
        let c = Config {
            enabled: true,
            token: v["bot_token"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("微信确认缺少凭据")?
                .into(),
            owner: v["ilink_user_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("微信确认缺少绑定用户")?
                .into(),
            base_url: safe_base(v["baseurl"].as_str().unwrap_or(BASE))?,
            since: data::now(),
            generation: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        };
        save(app, &c)?;
        put(app, "login", Value::Null)?;
        put(app, "status", json!({"state":"connected"}))?;
    }
    Ok(())
}
fn inbound_text(c: &Config, m: &Value) -> Option<(String, String)> {
    if m["from_user_id"].as_str() != Some(&c.owner)
        || m["message_type"].as_i64() != Some(1)
        || m["room_id"].as_str().is_some_and(|s| !s.is_empty())
        || m["chat_room_id"].as_str().is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    if m["create_time_ms"].as_i64().is_some_and(|t| t < c.since) {
        return None;
    }
    let id = match &m["message_id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let text = m["item_list"]
        .as_array()?
        .iter()
        .filter(|x| x["type"] == 1)
        .filter_map(|x| x["text_item"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if id.is_empty() || text.trim().is_empty() || text.len() > 16000 {
        return None;
    }
    Some((id, text))
}
async fn receive(app: &Arc<App>, c: &Config) -> AppResult<()> {
    let v = request(
        &c.base_url,
        "getupdates",
        Some(&c.token),
        Some(json!({"get_updates_buf":c.cursor})),
        &[],
    )
    .await?;
    let _guard = app.control.lock().await;
    let mut live = config(app)?;
    if !live.enabled || live.generation != c.generation {
        return Ok(());
    }
    // Commit inbox before cursor: a crash can replay an id but cannot lose accepted input.
    if let Some(msgs) = v["msgs"].as_array() {
        for m in msgs {
            if let Some((id, text)) = inbound_text(c, m) {
                if let Some(t) = m["context_token"].as_str().filter(|s| !s.is_empty()) {
                    live.context = t.into();
                }
                app.db
                    .lock()
                    .execute(
                        "INSERT OR IGNORE INTO weixin_inbox VALUES(?,?,?,NULL,?)",
                        params![
                            format!("{}:{id}", c.owner),
                            uuid::Uuid::new_v4().to_string(),
                            text,
                            c.generation
                        ],
                    )
                    .map_err(err)?;
            }
        }
    }
    if let Some(cursor) = v["get_updates_buf"].as_str() {
        live.cursor = cursor.into();
    }
    save(app, &live)?;
    report(app, &Ok(()));
    Ok(())
}
async fn respond(app: &Arc<App>, c: &Config) -> AppResult<()> {
    if app.dialogue.available_permits() == 0 {
        return Ok(());
    }
    let row:Option<(String,String)>=app.db.lock().query_row("SELECT request,content FROM weixin_inbox WHERE reply IS NULL AND generation=? ORDER BY rowid LIMIT 1",[&c.generation],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
    if let Some((request, text)) = row {
        // Recover an accepted request after a crash without generating twice.
        let recovered: Option<String> = app
            .db
            .lock()
            .query_row(
                "SELECT id FROM messages WHERE reply_to=? AND role='assistant'",
                [&request],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let reply = if let Some(id) = recovered {
            id
        } else {
            let result = service::accept_chat(
                app.clone(),
                service::ChatInput {
                    text,
                    request_id: request.clone(),
                },
            )
            .await?;
            result["message_id"]
                .as_str()
                .ok_or("微信回复关联失败")?
                .into()
        };
        app.db
            .lock()
            .execute(
                "UPDATE weixin_inbox SET reply=?,content='' WHERE request=?",
                params![reply, request],
            )
            .map_err(err)?;
    }
    Ok(())
}
async fn deliver(app: &Arc<App>, c: &Config) -> AppResult<()> {
    let _guard = app.control.lock().await;
    let live = config(app)?;
    if !live.enabled || live.generation != c.generation || live.context.is_empty() {
        return Ok(());
    }
    let primary = app.router.config().primary_channel == PrimaryChannel::Weixin;
    let row:Option<(String,String)>=app.db.lock().query_row("SELECT m.id,CASE WHEN m.status='failed' THEN '本次回复未完成：'||COALESCE(m.error,'请重试') ELSE m.content END FROM messages m LEFT JOIN weixin_delivery d ON d.id=m.id WHERE d.id IS NULL AND m.role='assistant' AND m.status IN ('done','failed') AND (trim(m.content)!='' OR m.status='failed') AND (m.id IN (SELECT reply FROM weixin_inbox WHERE generation=?) OR (? AND m.mode='proactive' AND m.created_at>=?)) ORDER BY m.created_at LIMIT 1",params![c.generation,primary,c.since.max(data::now()-300_000)],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
    if let Some((id, text)) = row {
        app.db
            .lock()
            .execute(
                "INSERT INTO weixin_delivery VALUES(?,'sending',NULL)",
                [&id],
            )
            .map_err(err)?;
        let result: AppResult<()> = async {
            let chars:Vec<char>=text.chars().collect();
            for (index,chunk) in chars.chunks(2000).enumerate(){
                let content:String=chunk.iter().collect();
                request(&live.base_url,"sendmessage",Some(&live.token),Some(json!({"msg":{"from_user_id":"","to_user_id":live.owner,"client_id":format!("{id}-{index}"),"message_type":2,"message_state":2,"item_list":[{"type":1,"text_item":{"text":content}}],"context_token":live.context}})),&[]).await?;
            }
            Ok(())
        }.await;
        app.db
            .lock()
            .execute(
                "UPDATE weixin_delivery SET status=?,error=? WHERE id=?",
                params![
                    if result.is_ok() { "sent" } else { "unknown" },
                    result.as_ref().err(),
                    id
                ],
            )
            .map_err(err)?;
        result?;
    }
    Ok(())
}
pub fn start(app: Arc<App>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let receiver = async {
            loop {
                let result = async {
                    let c = config(&app)?;
                    if c.enabled {
                        receive(&app, &c).await
                    } else {
                        poll_login(&app).await
                    }
                }
                .await;
                if result.is_err() {
                    report(&app, &result);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        };
        let sender = async {
            loop {
                if let Ok(c) = config(&app) {
                    if c.enabled {
                        let result = async {
                            respond(&app, &c).await?;
                            deliver(&app, &c).await
                        }
                        .await;
                        if result.is_err() {
                            report(&app, &result);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        };
        tokio::join!(receiver, sender);
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_host_and_owner() {
        assert!(safe_base(BASE).is_ok());
        for s in [
            "http://ilinkai.weixin.qq.com",
            "https://weixin.qq.com.evil.test",
            "https://u:p@ilinkai.weixin.qq.com",
            "https://ilinkai.weixin.qq.com/private",
        ] {
            assert!(safe_base(s).is_err());
        }
        let c = Config {
            owner: "owner".into(),
            since: 100,
            ..Default::default()
        };
        let mut m = json!({"message_id":12,"from_user_id":"owner","message_type":1,"create_time_ms":101,"item_list":[{"type":1,"text_item":{"text":"你好"}}]});
        assert_eq!(inbound_text(&c, &m), Some(("12".into(), "你好".into())));
        m["from_user_id"] = json!("stranger");
        assert!(inbound_text(&c, &m).is_none());
        m["from_user_id"] = json!("owner");
        m["room_id"] = json!("group");
        assert!(inbound_text(&c, &m).is_none());
    }
}
