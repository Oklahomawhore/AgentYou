use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use std::sync::{atomic::AtomicUsize, Mutex as StdMutex};
use tempfile::TempDir;
use tower::ServiceExt;

const TEST_KEY: &str = "sk-test-local-fixture-only";
#[derive(Default)]
struct Fake {
    requests: StdMutex<Vec<Value>>,
    calls: AtomicUsize,
    jev_next: StdMutex<String>,
    heartbeat_action: StdMutex<String>,
    jev_transient: AtomicUsize,
    slow_background: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
async fn fake_completion(
    State(fake): State<Arc<Fake>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(
        headers.get("authorization").unwrap(),
        format!("Bearer {TEST_KEY}").as_str()
    );
    fake.calls.fetch_add(1, Ordering::SeqCst);
    fake.requests.lock().unwrap().push(body.clone());
    if body["model"] == "bad-key" {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":{"message":format!("Invalid {TEST_KEY}")}})),
        )
            .into_response();
    }
    if body["model"] == "no-money" {
        return (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({"error":{"message":"insufficient credits"}})),
        )
            .into_response();
    }
    if body["model"] == "slow" {
        fake.entered.notify_one();
        fake.release.notified().await;
    }
    if body["max_tokens"].as_u64().unwrap_or(0) < 128 {
        return Json(json!({"choices":[{"message":{"role":"assistant","content":""},"finish_reason":"length"}],"usage":{"total_tokens":32}})).into_response();
    }
    if body["messages"][0]["content"]
        .as_str()
        .is_some_and(|s| s.contains("你在主循环中提出目标提案"))
    {
        return Json(json!({"choices":[{"message":{"content":json!({"objective":"finish a useful fixture","kind":"practical","why":"explicit fixture evidence","next_step":"write fixture","success_criteria":"fixture exists","evidence":"not completed"}).to_string()},"finish_reason":"stop"}],"usage":{"total_tokens":100}})).into_response();
    }
    let schema = body
        .pointer("/response_format/json_schema/name")
        .and_then(Value::as_str);
    let messages = body["messages"].as_array().unwrap();
    if fake.slow_background.load(Ordering::SeqCst)
        && messages.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|s| s.contains("Jev 已决定主动联系用户"))
        })
    {
        fake.entered.notify_one();
        fake.release.notified().await;
    }
    let tool_reply = messages.iter().any(|m| m["role"] == "tool");
    let remember = messages.iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|s| s.contains("remember-fixture"))
    });
    let workspace_fixture = messages.iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|s| s.contains("workspace-tool-fixture"))
    });
    let message = if workspace_fixture && body["tools"].is_array() {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"tool_fixture","type":"function","function":{"name":"Write","arguments":"{\"path\":\"fixture.txt\",\"content\":\"tool pipeline works\"}"}}]})
    } else if remember && body.get("tools").is_some() && !tool_reply {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_fixture_1","type":"function","function":{"name":"remember","arguments":"{\"content\":\"用户喜欢简洁的回答。\"}"}}]})
    } else if schema == Some("appraisal") {
        json!({"role":"assistant","content":json!({"benefit":0.96,"novelty":0.95,"interruption":0.02,"interest":0.9,"goal_congruence":0.85,"surprise":0.5,"evidence_sufficient":0.96}).to_string()})
    } else if body["model"] == "empty" {
        json!({"role":"assistant","content":""})
    } else {
        json!({"role":"assistant","content":"这是本地契约测试生成的结果。"})
    };
    Json(json!({"id":"fixture","model":"fixture-model","choices":[{"message":message,"finish_reason":"stop"}],"usage":{"total_tokens":42,"cost":0.00001}})).into_response()
}
async fn fake_jev(
    State(fake): State<Arc<Fake>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["authorization"], format!("Bearer {TEST_KEY}"));
    assert_eq!(body["model"], "jev");
    for field in ["messages", "stream", "temperature", "max_tokens"] {
        assert!(body.get(field).is_none());
    }
    fake.requests.lock().unwrap().push(body.clone());
    if body["state"]["transport_fixture"] == true
        && fake.jev_transient.fetch_add(1, Ordering::SeqCst) == 0
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"temporary fixture"})),
        )
            .into_response();
    }
    if body["state"]["disabled_fixture"] == true {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{"message":"System One endpoint is disabled"}})),
        )
            .into_response();
    }
    let override_next = fake.jev_next.lock().unwrap().clone();
    if override_next == "malformed" {
        return Json(json!({"answers":{"next":{"type":"choice","choice":"launch_shell"}}}))
            .into_response();
    }
    let mut answers = serde_json::Map::new();
    for (name, q) in body["questions"].as_object().unwrap() {
        let a = if q["type"] == "noul" {
            json!({"type":"noul","noul":if name=="interruption"{0.02}else{0.96}})
        } else {
            let criteria = q["criteria"].as_object().unwrap();
            let heartbeat_action = fake.heartbeat_action.lock().unwrap().clone();
            let selected = if name == "goal" {
                "adopt"
            } else if criteria.contains_key("organize") {
                if heartbeat_action.is_empty() {
                    "wait"
                } else {
                    heartbeat_action.as_str()
                }
            } else if criteria.contains_key("review_memory") {
                "review_memory"
            } else if criteria.contains_key("supersede") {
                if body["state"].to_string().contains("correction-fixture") {
                    "supersede"
                } else {
                    "keep"
                }
            } else if criteria.contains_key("adopt_concise") {
                if body["state"]["latest_user_message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("adaptive-concise")
                {
                    "adopt_concise"
                } else if body["state"]["latest_user_message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("adaptive-observe")
                {
                    "observe_detailed"
                } else {
                    "keep"
                }
            } else if criteria.contains_key("execute") {
                if body["state"].to_string().contains("reject-tool-fixture") {
                    "skip"
                } else {
                    "execute"
                }
            } else if criteria.contains_key("keep") {
                "keep"
            } else if name.starts_with('m') && criteria.contains_key("discard") {
                if body["state"].to_string().contains("discard-fixture") {
                    "discard"
                } else {
                    "user_note"
                }
            } else if criteria.contains_key("curiosity_question") {
                "curiosity_question"
            } else if criteria.contains_key("speak") {
                "speak"
            } else if criteria.contains_key("reminder") {
                "task"
            } else if criteria.contains_key("unclear") {
                "0"
            } else if override_next == "search_memory"
                && body["state"]["executed_steps"]
                    .to_string()
                    .contains("search_memory")
            {
                "reply"
            } else if !override_next.is_empty() {
                override_next.as_str()
            } else if criteria.contains_key("notify") {
                "notify"
            } else {
                "reply"
            };
            let probabilities: serde_json::Map<String, Value> = criteria
                .keys()
                .map(|key| (key.clone(), json!(if key == selected { 1.0 } else { 0.0 })))
                .collect();
            json!({"type":"choice","choice":selected,"probabilities":probabilities,"confidence":1.0})
        };
        answers.insert(name.clone(), a);
    }
    Json(json!({"model":"jev","answers":answers,"usage":{"total_tokens":24}})).into_response()
}
struct Harness {
    dir: TempDir,
    app: Arc<App>,
    fake: Arc<Fake>,
    server: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn new() -> Self {
        let fake = Arc::new(Fake::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = Router::new()
            .route("/chat/completions", post(fake_completion))
            .route("/systemone", post(fake_jev))
            .route(
                "/models",
                get(|headers: axum::http::HeaderMap| async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        format!("Bearer {TEST_KEY}").as_str()
                    );
                    Json(json!({"data":[{"id":"gpt-5.4-mini","object":"model"}]}))
                }),
            )
            .with_state(fake.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(&dir.path().join("app.sqlite")).unwrap());
        db.lock().execute("UPDATE agent_drive SET payload=? WHERE id=1",[json!({"status":"active","objective":"fixture useful progress","success_criteria":"fixture delivered","next_step":"share fixture result"}).to_string()]).unwrap();
        let config = Settings {
            api_key: TEST_KEY.into(),
            ..Settings::default()
        };
        db.save_settings(&config).unwrap();
        let provider = Arc::new(
            OpenRouter::with_endpoint(
                Arc::new(RwLock::new(config.clone())),
                db.clone(),
                format!("http://{addr}"),
            )
            .unwrap(),
        );
        let mind = Runtime::spawn_with_timeout(
            Store::open(
                dir.path().join("mind.sqlite"),
                Persona::default(),
                data::now(),
            )
            .unwrap(),
            provider.clone(),
            Arc::new(SystemClock),
            Duration::from_secs(10),
        )
        .unwrap();
        mind.set_guards(config.guards).await.unwrap();
        db.lock().execute_batch("CREATE TABLE IF NOT EXISTS workspace_events(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,payload TEXT NOT NULL)").unwrap();
        let app = Arc::new(App {
            workspace: Arc::new(
                crate::workspace::Workspace::new(&dir.path().join("workspace")).unwrap(),
            ),
            db,
            router: provider,
            mind,
            token: "test-session-token".into(),
            port: 4317,
            epoch: AtomicU64::new(0),
            dialogue_epoch: AtomicU64::new(0),
            control: Mutex::new(()),
            dialogue: Arc::new(Semaphore::new(1)),
            cancel_signal: tokio::sync::watch::channel(0).0,
        });
        crate::bootstrap::init(&app).unwrap();
        crate::heartbeat::init(&app).unwrap();
        crate::feishu::init(&app).unwrap();
        Self {
            dir,
            app,
            fake,
            server,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:4317")
            .header("x-yourself-token", &self.app.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app_router(self.app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(json!(String::from_utf8_lossy(&bytes))),
        )
    }
    async fn wait_chat(&self) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while self.app.dialogue.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn close(self) {
        self.app.mind.shutdown().await.unwrap();
        self.server.abort();
    }
}
#[tokio::test]
async fn settings_redact_key_and_preserve_blank_updates() {
    let h = Harness::new().await;
    let (status, value) = h.request("GET", "/api/state", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!value.to_string().contains(TEST_KEY));
    assert_eq!(value["settings"]["has_key"], true);
    let mut update = h.app.router.config().public();
    update.as_object_mut().unwrap().remove("has_key");
    update.as_object_mut().unwrap().remove("profiles");
    update["api_key"] = json!("");
    assert_eq!(
        h.request("POST", "/api/settings", update).await.0,
        StatusCode::OK
    );
    assert_eq!(h.app.router.config().api_key, TEST_KEY);
    assert_eq!(h.app.db.settings().unwrap().api_key, TEST_KEY);
    h.close().await;
}
#[tokio::test]
async fn cross_origin_host_and_missing_session_are_rejected() {
    let h = Harness::new().await;
    for (host, origin, token, expected) in [
        (
            "evil.test:4317",
            None,
            Some("test-session-token"),
            StatusCode::FORBIDDEN,
        ),
        (
            "127.0.0.1:4317",
            Some("https://evil.test"),
            Some("test-session-token"),
            StatusCode::FORBIDDEN,
        ),
        ("127.0.0.1:4317", None, None, StatusCode::UNAUTHORIZED),
    ] {
        let mut request = axum::http::Request::builder()
            .uri("/api/state")
            .header("host", host);
        if let Some(o) = origin {
            request = request.header("origin", o);
        }
        if let Some(t) = token {
            request = request.header("x-yourself-token", t);
        }
        let response = app_router(h.app.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    h.close().await;
}
#[tokio::test]
async fn real_http_contract_chat_tools_and_idempotency() {
    let h = Harness::new().await;
    let request_id = data::id();
    let input = json!({"text":"请记住 remember-fixture","request_id":request_id});
    assert_eq!(
        h.request("POST", "/api/chat", input.clone()).await.0,
        StatusCode::OK
    );
    h.wait_chat().await;
    let messages = h.app.db.messages(20).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["status"], "done");
    assert_eq!(h.app.db.memories().unwrap().len(), 1);
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), 2);
    let duplicate = h.request("POST", "/api/chat", input).await.1;
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), 2);
    assert_eq!(h.app.db.usage().unwrap()["tokens"], 84);
    h.close().await;
}
#[tokio::test]
async fn no_key_pause_and_budget_block_before_network() {
    let h = Harness::new().await;
    let mut config = h.app.router.config();
    config.api_key.clear();
    *h.app.router.settings.write().unwrap() = config.clone();
    assert_eq!(
        h.request(
            "POST",
            "/api/chat",
            json!({"text":"hi","request_id":data::id()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    config.api_key = TEST_KEY.into();
    config.guards.paused = true;
    *h.app.router.settings.write().unwrap() = config.clone();
    assert!(h
        .app
        .router
        .complete(&config.model, vec![], "test", None, None, false)
        .await
        .is_err());
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), 0);
    config.guards.paused = false;
    config.daily_call_limit = 1;
    *h.app.router.settings.write().unwrap() = config.clone();
    assert!(h
        .app
        .router
        .complete(
            &config.model,
            vec![json!({"role":"user","content":"hi"})],
            "test",
            None,
            None,
            false
        )
        .await
        .is_ok());
    assert!(h
        .app
        .router
        .complete(&config.model, vec![], "test", None, None, false)
        .await
        .unwrap_err()
        .contains("上限"));
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), 1);
    h.close().await;
}
#[tokio::test]
async fn error_key_redacted_credit_error_and_empty_response_are_actionable() {
    let h = Harness::new().await;
    for (model, expected) in [
        ("bad-key", "API Key"),
        ("no-money", "余额不足"),
        ("empty", "空内容"),
    ] {
        let error = h
            .app
            .router
            .complete(
                model,
                vec![json!({"role":"user","content":"hi"})],
                "test",
                None,
                None,
                false,
            )
            .await
            .unwrap_err();
        assert!(error.contains(expected));
        assert!(!error.contains(TEST_KEY));
    }
    assert!(!h
        .app
        .db
        .calls()
        .unwrap()
        .iter()
        .any(|c| c.to_string().contains(TEST_KEY)));
    h.close().await;
}
#[tokio::test]
async fn pause_during_generation_discards_result() {
    let h = Harness::new().await;
    h.app.router.settings.write().unwrap().model = "slow".into();
    h.request(
        "POST",
        "/api/chat",
        json!({"text":"慢一点","request_id":data::id()}),
    )
    .await;
    h.fake.entered.notified().await;
    assert_eq!(
        h.request("POST", "/api/pause", json!({"paused":true}))
            .await
            .0,
        StatusCode::OK
    );
    h.fake.release.notify_one();
    h.wait_chat().await;
    let messages = h.app.db.messages(10).unwrap();
    assert_eq!(messages[1]["status"], "failed");
    assert_eq!(messages[1]["content"], "");
    h.close().await;
}
#[tokio::test]
async fn work_reflection_and_local_outbox_complete_end_to_end() {
    let h = Harness::new().await;
    let mut config = h.app.router.config();
    config.guards.kinds[0].consent = true;
    config.guards.kinds[0].cooldown_ms = 0;
    *h.app.router.settings.write().unwrap() = config.clone();
    h.app.mind.set_guards(config.guards).await.unwrap();
    h.app
        .db
        .add_memory("user_note", "用户喜欢直接的交流。", &[])
        .unwrap();
    let key = h
        .app
        .db
        .create_job("reflection", "整理交流偏好", data::now())
        .unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    run_job(&h.app, &job).await.unwrap();
    assert_eq!(h.app.db.jobs().unwrap()[0]["status"], "done");
    assert_eq!(h.app.db.memories().unwrap().len(), 2);
    let messages = h.app.db.messages(10).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["mode"], "proactive");
    h.app
        .db
        .deliver(&format!("job-{key}"), "duplicate", &key)
        .unwrap();
    assert_eq!(h.app.db.messages(10).unwrap().len(), 1);
    assert!(h
        .fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.pointer("/response_format/json_schema/name") == Some(&json!("appraisal"))));
    h.close().await;
}
#[tokio::test]
async fn deleting_memory_cascades_derived_outputs_and_runtime_evidence() {
    let h = Harness::new().await;
    let memory = h
        .app
        .db
        .add_memory("user_note", "请删除的秘密", &[])
        .unwrap();
    let job_id = h.app.db.create_job("task", "整理", data::now()).unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    run_job(&h.app, &job).await.unwrap();
    assert_eq!(h.app.mind.decisions().await.unwrap().len(), 1);
    let reflection = h
        .app
        .db
        .add_memory("reflection", "派生摘要", std::slice::from_ref(&memory))
        .unwrap();
    let reply = h.app.db.begin_chat(&data::id(), "hi").unwrap().unwrap();
    h.app
        .db
        .complete_chat(&reply, "派生回答", &[reflection])
        .unwrap();
    let response = h
        .request("DELETE", &format!("/api/memories/{memory}"), json!({}))
        .await;
    assert_eq!(response.0, StatusCode::OK, "{}", response.1);
    assert!(h.app.db.memories().unwrap().is_empty());
    assert!(!h
        .app
        .db
        .messages(10)
        .unwrap()
        .iter()
        .any(|m| m["id"] == reply));
    assert_eq!(
        h.app
            .db
            .jobs()
            .unwrap()
            .iter()
            .find(|j| j["id"] == job_id)
            .unwrap()["status"],
        "cancelled"
    );
    assert!(h.app.mind.decisions().await.unwrap().is_empty());
    h.close().await;
}
#[tokio::test]
async fn scheduled_jobs_and_recurring_goal_survive_restart_without_busy_loop() {
    let h = Harness::new().await;
    let mut config = h.app.router.config();
    config.exploration_goal = "研究叙事方法".into();
    config.exploration_interval_minutes = 30;
    h.app.db.schedule_exploration(&config).unwrap();
    h.app.db.schedule_exploration(&config).unwrap();
    assert_eq!(h.app.db.jobs().unwrap().len(), 1);
    let job = h.app.db.claim_job().unwrap().unwrap();
    h.app
        .db
        .finish_job(job["id"].as_str().unwrap(), "done", &[])
        .unwrap();
    h.app.db.schedule_exploration(&config).unwrap();
    assert_eq!(h.app.db.jobs().unwrap().len(), 1);
    h.app
        .db
        .create_job("reminder", "稍后", data::now() + 60_000)
        .unwrap();
    assert!(h.app.db.claim_job().unwrap().is_none());
    let path = h.dir.path().join("app.sqlite");
    h.app.mind.shutdown().await.unwrap();
    h.server.abort();
    drop(h.app);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(reopened.jobs().unwrap().len(), 2);
}
#[test]
fn restart_marks_ambiguous_calls_and_interrupted_work_without_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    let db = Database::open(&path).unwrap();
    db.begin_chat(&data::id(), "user request").unwrap();
    db.create_job("task", "work", data::now()).unwrap();
    db.claim_job().unwrap();
    db.reserve_call("work", "fixture", 10).unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.messages(10).unwrap()[1]["status"], "failed");
    assert_eq!(db.jobs().unwrap()[0]["status"], "failed");
    assert_eq!(db.calls().unwrap()[0]["status"], "unknown");
    assert!(db.claim_job().unwrap().is_none());
}

#[tokio::test]
async fn cancellation_releases_turn_and_marks_unreceipted_call_unknown() {
    let h = Harness::new().await;
    h.app.router.settings.write().unwrap().model = "slow".into();
    h.request(
        "POST",
        "/api/chat",
        json!({"text":"cancel fixture","request_id":data::id()}),
    )
    .await;
    h.fake.entered.notified().await;
    assert_eq!(
        h.request("POST", "/api/chat/cancel", json!({})).await.0,
        StatusCode::OK
    );
    h.wait_chat().await;
    assert_eq!(h.app.db.calls().unwrap()[0]["status"], "unknown");
    assert_eq!(h.app.db.messages(10).unwrap()[1]["status"], "failed");
    h.fake.release.notify_one();
    h.app.router.settings.write().unwrap().model = "fixture".into();
    assert_eq!(
        h.request(
            "POST",
            "/api/chat",
            json!({"text":"next turn","request_id":data::id()})
        )
        .await
        .0,
        StatusCode::OK
    );
    h.wait_chat().await;
    assert_eq!(h.app.db.messages(10).unwrap()[3]["status"], "done");
    h.close().await;
}

#[tokio::test]
async fn unfinished_erasure_is_recovered_before_runtime_start() {
    let h = Harness::new().await;
    let memory = h
        .app
        .db
        .add_memory("user_note", "private fixture", &[])
        .unwrap();
    h.app
        .db
        .create_job("task", "summarize", data::now())
        .unwrap();
    run_job(&h.app, &h.app.db.claim_job().unwrap().unwrap())
        .await
        .unwrap();
    h.app.db.forget_memory(&memory).unwrap();
    assert!(!h.app.db.pending_erasure().unwrap().is_empty());
    let path = h.dir.path().to_path_buf();
    h.app.mind.shutdown().await.unwrap();
    h.server.abort();
    drop(h.app);
    let restarted = App::open(&path, 4317).await.unwrap();
    assert!(restarted.db.pending_erasure().unwrap().is_empty());
    assert!(restarted.mind.decisions().await.unwrap().is_empty());
    restarted.mind.shutdown().await.unwrap();
}

#[test]
fn web_citations_preserve_sources_and_reject_unsafe_url_schemes() {
    let output = json!({"choices":[{"message":{"content":"研究结论。","annotations":[
        {"type":"url_citation","url_citation":{"title":"官方来源","url":"https://example.com/research"}},
        {"type":"url_citation","url_citation":{"title":"invalid","url":"javascript:alert(1)"}}
    ]}}]});
    let text = openrouter::content(&output).unwrap();
    assert!(text.contains("https://example.com/research"));
    assert!(!text.contains("javascript:"));
}

#[tokio::test]
async fn teamorouter_credentials_are_isolated_and_persisted() {
    let h = Harness::new().await;
    let mut update = h.app.router.config().public();
    update.as_object_mut().unwrap().remove("has_key");
    update.as_object_mut().unwrap().remove("profiles");
    update["provider"] = json!("teamorouter");
    update["model"] = json!("gpt-5.4-mini");
    update["decision_model"] = json!("gpt-5.4-mini");
    update["api_key"] = json!("");
    assert_eq!(
        h.request("POST", "/api/settings", update.clone()).await.0,
        StatusCode::OK
    );
    assert!(h.app.router.config().active_key().is_empty());
    assert!(h
        .app
        .router
        .complete("gpt-5.4-mini", vec![], "test", None, None, false)
        .await
        .is_err());
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), 0);
    update["api_key"] = json!(TEST_KEY);
    assert_eq!(
        h.request("POST", "/api/settings", update.clone()).await.0,
        StatusCode::OK
    );
    let result = h
        .app
        .router
        .complete(
            "gpt-5.4-mini",
            vec![],
            "test",
            Some(openrouter::json_format("appraisal", json!({}))),
            Some(json!([])),
            true,
        )
        .await;
    assert!(result.is_ok());
    let body = h.fake.requests.lock().unwrap()[0].clone();
    assert!(body.get("provider").is_none());
    assert!(body.get("plugins").is_none());
    assert!(body.get("response_format").is_some());
    assert!(body.get("tools").is_some());
    assert_eq!(
        h.app.router.models().await.unwrap()["models"][0]["id"],
        "gpt-5.4-mini"
    );
    assert_eq!(h.app.db.settings().unwrap().teamorouter.api_key, TEST_KEY);
    assert!(!h
        .app
        .router
        .config()
        .public()
        .to_string()
        .contains(TEST_KEY));
    update["clear_key"] = json!(true);
    assert_eq!(
        h.request("POST", "/api/settings", update).await.0,
        StatusCode::OK
    );
    assert!(h.app.router.config().teamorouter.api_key.is_empty());
    assert_eq!(h.app.router.config().api_key, TEST_KEY);
    h.close().await;
}

#[test]
fn legacy_settings_keep_openrouter_and_provider_endpoints_are_fixed() {
    let s: Settings = serde_json::from_value(
        json!({"api_key":TEST_KEY,"model":"original","decision_model":"judge"}),
    )
    .unwrap();
    assert!(s.provider == data::Provider::Openrouter);
    assert_eq!(s.active_model(), "original");
    assert_eq!(s.active_key(), TEST_KEY);
    assert!(s.teamorouter.api_key.is_empty());
    assert_eq!(
        data::Provider::Teamorouter.endpoint(),
        "https://api.teamorouter.com/v1"
    );
}

async fn use_jev(h: &Harness) {
    let mut config = h.app.router.config();
    config.provider = data::Provider::Teamorouter;
    config.teamorouter.api_key = TEST_KEY.into();
    config.teamorouter.model = "deepseek-flash".into();
    config.teamorouter.decision_model = "jev".into();
    for k in &mut config.guards.kinds {
        k.consent = true;
    }
    h.app.db.save_settings(&config).unwrap();
    *h.app.router.settings.write().unwrap() = config.clone();
    h.app.mind.set_guards(config.guards).await.unwrap();
}
#[tokio::test]
async fn jev_native_routes_reply_to_deepseek_and_wait_is_silent() {
    let h = Harness::new().await;
    use_jev(&h).await;
    assert_eq!(
        h.request(
            "POST",
            "/api/chat",
            json!({"request_id":data::id(),"text":"你好"})
        )
        .await
        .0,
        StatusCode::OK
    );
    h.wait_chat().await;
    let requests = h.fake.requests.lock().unwrap().clone();
    assert_eq!(requests[0]["model"], "jev");
    assert_eq!(requests[1]["model"], "jev");
    assert_eq!(requests[2]["model"], "jev");
    assert_eq!(requests[3]["model"], "deepseek-flash");
    assert!(requests[3].get("tools").is_none());
    *h.fake.jev_next.lock().unwrap() = "wait".into();
    let before = h.fake.calls.load(Ordering::SeqCst);
    h.request(
        "POST",
        "/api/chat",
        json!({"request_id":data::id(),"text":"先不用回复"}),
    )
    .await;
    h.wait_chat().await;
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), before);
    assert_eq!(
        h.app.db.messages(20).unwrap().last().unwrap()["status"],
        "skipped"
    );
    *h.fake.jev_next.lock().unwrap() = "malformed".into();
    h.request(
        "POST",
        "/api/chat",
        json!({"request_id":data::id(),"text":"坏判定"}),
    )
    .await;
    h.wait_chat().await;
    assert_eq!(h.fake.calls.load(Ordering::SeqCst), before);
    assert_eq!(
        h.app.db.messages(20).unwrap().last().unwrap()["status"],
        "failed"
    );
    h.close().await;
}
#[tokio::test]
async fn jev_selects_message_kind_and_appraises_via_native_api() {
    let h = Harness::new().await;
    use_jev(&h).await;
    h.app
        .db
        .create_job("research", "分析一个实际问题", data::now())
        .unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    run_job(&h.app, &job).await.unwrap();
    assert_eq!(
        h.app.db.plans().unwrap()[0]["plan"]["kind"],
        "curiosity_question"
    );
    assert_eq!(
        h.app
            .db
            .lock()
            .query_row("SELECT kind FROM notification_kinds", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "curiosity_question"
    );
    assert!(h
        .fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r["questions"]["benefit"]["type"] == "noul"));
    h.close().await;
}

#[tokio::test]
async fn jev_replans_after_tool_result_and_selected_kind_needs_consent() {
    let h = Harness::new().await;
    use_jev(&h).await;
    *h.fake.jev_next.lock().unwrap() = "search_memory".into();
    h.request(
        "POST",
        "/api/chat",
        json!({"request_id":data::id(),"text":"我平时喜欢简短回答 remember-fixture"}),
    )
    .await;
    h.wait_chat().await;
    assert_eq!(h.app.db.memories().unwrap().len(), 1);
    assert_eq!(
        h.app.db.messages(20).unwrap().last().unwrap()["status"],
        "done"
    );
    assert_eq!(h.app.db.plans().unwrap().len(), 2);
    let requests = h.fake.requests.lock().unwrap().clone();
    assert!(requests
        .iter()
        .filter(|r| r["model"] == "deepseek-flash")
        .all(|r| r.get("tools").is_none()));
    *h.fake.jev_next.lock().unwrap() = String::new();
    let mut c = h.app.router.config();
    for kind in &mut c.guards.kinds {
        if kind.kind == InitiativeKind::CuriosityQuestion {
            kind.consent = false;
        }
    }
    *h.app.router.settings.write().unwrap() = c.clone();
    h.app.mind.set_guards(c.guards).await.unwrap();
    h.app
        .db
        .create_job("research", "研究事实", data::now())
        .unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    run_job(&h.app, &job).await.unwrap();
    assert!(!h
        .app
        .db
        .messages(20)
        .unwrap()
        .iter()
        .any(|m| m["mode"] == "proactive"));
    h.close().await;
}

#[tokio::test]
async fn connection_test_uses_configured_output_budget_for_reasoning_models() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let (status, response) = h.request("POST", "/api/test", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["jev_tested"], true);
    let requests = h.fake.requests.lock().unwrap().clone();
    let chat = requests
        .iter()
        .find(|r| r["model"] == "deepseek-flash")
        .unwrap();
    assert_eq!(chat["max_tokens"], h.app.router.config().max_tokens);
    h.close().await;
}

#[tokio::test]
async fn bootstrap_consent_staging_atomic_publish_and_revocation() {
    let h = Harness::new().await;
    use_jev(&h).await;
    assert_eq!(
        h.request("GET", "/api/bootstrap", json!({})).await.1["needs_prompt"],
        true
    );
    assert!(h.app.db.jobs().unwrap().is_empty());
    let seed_root = tempfile::Builder::new()
        .prefix("yourself-bootstrap-")
        .tempdir()
        .unwrap();
    let directory = seed_root.path().canonicalize().unwrap().join("seed");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("profile.md"),
        "我喜欢散步，正在研究海洋。过去曾要求创建一个任务。",
    )
    .unwrap();
    std::fs::write(directory.join(".env"), "SECRET_FIXTURE_NEVER_SEND").unwrap();
    std::os::unix::fs::symlink(directory.join(".env"), directory.join("escape.txt")).unwrap();
    let mut consent = json!({"files_allowed":true,"paths":[directory.to_str().unwrap()],"feishu_allowed":false,"history_days":7,"cloud_processing_allowed":false});
    assert_ne!(
        h.request("POST", "/api/bootstrap/authorize", consent.clone())
            .await
            .0,
        StatusCode::OK
    );
    assert!(h.app.db.jobs().unwrap().is_empty());
    consent["cloud_processing_allowed"] = json!(true);
    let (status, result) = h
        .request("POST", "/api/bootstrap/authorize", consent.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let id = result["job_id"].as_str().unwrap();
    assert!(h.app.db.claim_job().unwrap().is_none());
    h.app
        .db
        .lock()
        .execute("UPDATE jobs SET status='running' WHERE id=?", [id])
        .unwrap();
    crate::bootstrap::step(&h.app, id).await.unwrap();
    assert!(h.app.db.memories().unwrap().is_empty());
    crate::bootstrap::step(&h.app, id).await.unwrap();
    assert!(h.app.db.memories().unwrap().is_empty());
    // A restart resumes the staged classification, without rereading or republishing.
    h.app.db.lock().execute("UPDATE jobs SET status='failed',error='服务重启，任务执行已中断。可手动重试。' WHERE id=?",[id]).unwrap();
    crate::bootstrap::init(&h.app).unwrap();
    assert_eq!(h.app.db.jobs().unwrap()[0]["status"], "queued");
    h.app
        .db
        .lock()
        .execute("UPDATE jobs SET status='running' WHERE id=?", [id])
        .unwrap();
    crate::bootstrap::step(&h.app, id).await.unwrap();
    assert_eq!(h.app.db.memories().unwrap().len(), 1);
    assert_eq!(h.app.db.jobs().unwrap().len(), 1); // Historical requests never become jobs.
    assert_eq!(h.app.db.jobs().unwrap()[0]["status"], "done");
    let public = crate::bootstrap::public(&h.app).unwrap();
    assert_eq!(public["runs"][0]["processed"], 1);
    assert!(!h
        .fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.to_string().contains("SECRET_FIXTURE_NEVER_SEND")));
    assert!(crate::bootstrap::step(&h.app, id).await.is_err());
    assert_eq!(h.app.db.memories().unwrap().len(), 1);
    let (_, second) = h.request("POST", "/api/bootstrap/authorize", consent).await;
    let second = second["job_id"].as_str().unwrap();
    h.app
        .db
        .lock()
        .execute("UPDATE jobs SET status='running' WHERE id=?", [second])
        .unwrap();
    crate::bootstrap::step(&h.app, second).await.unwrap();
    assert_eq!(
        h.request("POST", "/api/bootstrap/revoke", json!({"job_id":second}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(crate::bootstrap::step(&h.app, second).await.is_err());
    assert_ne!(
        h.request("POST", &format!("/api/jobs/{second}/retry"), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let bytes: i64 = h
        .app
        .db
        .lock()
        .query_row(
            "SELECT COALESCE(SUM(length(body)),0) FROM bootstrap_chunks",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bytes, 0);
    h.close().await;
}

#[tokio::test]
async fn jev_retention_discard_and_source_dedup_and_retrieval() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let discarded = h
        .app
        .router
        .classify_memory("discard-fixture 临时闲聊", "live_user_message", json!([]))
        .await
        .unwrap();
    assert_eq!(
        h.app
            .db
            .store_classified("one", "测试来源", &discarded, &[])
            .unwrap(),
        0
    );
    let items = h
        .app
        .router
        .classify_memory("海洋保护是我长期研究的方向", "live_user_message", json!([]))
        .await
        .unwrap();
    assert_eq!(
        h.app
            .db
            .store_classified("two", "测试来源", &items, &[])
            .unwrap(),
        1
    );
    assert_eq!(
        h.app
            .db
            .store_classified("two", "测试来源", &items, &[])
            .unwrap(),
        0
    );
    for i in 0..30 {
        h.app
            .db
            .add_memory("user_note", &format!("无关内容 {i}"), &[])
            .unwrap();
    }
    let found = h.app.db.relevant_memories("海洋保护", 1).unwrap();
    assert_eq!(found[0]["content"], "海洋保护是我长期研究的方向");
    assert_eq!(found[0]["source"], "测试来源");
    h.close().await;
}

#[tokio::test]
async fn adaptive_profiles_observe_adopt_context_versions_and_restore() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let updates = h
        .app
        .router
        .adapt_profiles("adaptive-observe", json!([]))
        .await
        .unwrap();
    h.app
        .db
        .commit_adaptation("observation-one", &updates)
        .unwrap();
    let p = h.app.db.adaptive_profiles().unwrap();
    assert_eq!(p["self"]["preferences"]["expression"], "balanced");
    assert_eq!(p["self"]["observations"]["expression"]["count"], 1);
    let original = p["self"]["version"].as_str().unwrap().to_owned();
    h.request(
        "POST",
        "/api/chat",
        json!({"request_id":data::id(),"text":"adaptive-concise 我希望我们以后都简洁直接地交流。"}),
    )
    .await;
    h.wait_chat().await;
    let p = h.app.db.adaptive_profiles().unwrap();
    assert_eq!(p["self"]["preferences"]["expression"], "concise");
    assert_eq!(p["user"]["preferences"]["expression"], "concise");
    assert!(p["self"]["observations"].get("expression").is_none());
    let (context, _) = h.app.dialogue_context().unwrap();
    assert!(context[0]["content"].as_str().unwrap().contains("concise"));
    let requests = h.fake.requests.lock().unwrap().clone();
    assert!(requests
        .iter()
        .any(
            |r| r["state"]["adaptive_profiles"]["self"]["preferences"]["expression"] == "concise"
        ));
    let profiles = h
        .app
        .db
        .memories()
        .unwrap()
        .into_iter()
        .filter(|m| m["kind"] == "profile_self")
        .count();
    assert_eq!(profiles, 2);
    assert_eq!(
        h.request(
            "POST",
            &format!("/api/profiles/{original}/restore"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h.app.db.adaptive_profiles().unwrap()["self"]["preferences"]["expression"],
        "balanced"
    );
    h.app.db.forget_memory(&original).unwrap();
    assert_eq!(
        h.app.db.adaptive_profiles().unwrap()["self"]["version"],
        Value::Null
    );
    h.close().await;
}

#[tokio::test]
async fn corrected_knowledge_preserves_history_but_excludes_stale_context() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let old = h
        .app
        .db
        .add_memory("user_note", "我喜欢详细的回答", &[])
        .unwrap();
    let existing = h.app.db.relevant_memories("回答", 20).unwrap();
    let items = h
        .app
        .router
        .classify_memory(
            "correction-fixture 我现在更喜欢简短回答",
            "live_user_message",
            json!(existing),
        )
        .await
        .unwrap();
    assert_eq!(items[0].replaces, vec![old.clone()]);
    h.app
        .db
        .store_classified("new-correction", "实时对话", &items, &[])
        .unwrap();
    let current = h.app.db.relevant_memories("回答", 20).unwrap();
    assert!(!current.iter().any(|m| m["id"] == old));
    assert!(h
        .app
        .db
        .memories()
        .unwrap()
        .iter()
        .any(|m| m["id"] == old && m["superseded"] == true));
    h.close().await;
}

#[tokio::test]
async fn jev_retries_temporary_failure_once_with_separate_receipts() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let result = h
        .app
        .router
        .system_one(
            json!({"transport_fixture":true}),
            json!({"next":crate::jev::choice("Test",json!({"reply":"Reply","wait":"Wait"}))}),
            "jev_plan",
        )
        .await
        .unwrap();
    assert_eq!(result["answers"]["next"]["choice"], "reply");
    assert_eq!(h.fake.jev_transient.load(Ordering::SeqCst), 2);
    assert_eq!(h.app.db.usage().unwrap()["calls"], 2);
    let calls = h.app.db.calls().unwrap();
    assert!(calls.iter().any(|c| c["status"] == "failed"));
    assert!(calls.iter().any(|c| c["status"] == "done"));
    h.close().await;
}

#[tokio::test]
async fn heartbeat_due_wait_organization_and_disable_are_durable() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    crate::heartbeat::tick(&h.app, due - 1).await.unwrap();
    assert!(h.fake.requests.lock().unwrap().is_empty());
    crate::heartbeat::tick(&h.app, due).await.unwrap();
    assert_eq!(
        crate::heartbeat::public(&h.app).unwrap()["runs"][0]["action"],
        "wait"
    );
    let before = h.fake.requests.lock().unwrap().len();
    crate::heartbeat::init(&h.app).unwrap();
    crate::heartbeat::tick(&h.app, due + 1).await.unwrap();
    assert_eq!(h.fake.requests.lock().unwrap().len(), before);
    *h.fake.heartbeat_action.lock().unwrap() = "organize".into();
    crate::heartbeat::tick(&h.app, due + 30 * 60000)
        .await
        .unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    assert_eq!(job["kind"], "self_review");
    run_job(&h.app, &job).await.unwrap();
    assert!(!h
        .app
        .db
        .messages(20)
        .unwrap()
        .iter()
        .any(|m| m["mode"] == "proactive"));
    let next = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    crate::heartbeat::tick(&h.app, next).await.unwrap();
    let (status, _) = h
        .request(
            "POST",
            "/api/heartbeat",
            json!({"enabled":false,"interval_minutes":30,"public_topics":[]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h.app.db.claim_job().unwrap().is_none());
    let before = h.fake.requests.lock().unwrap().len();
    crate::heartbeat::tick(&h.app, next + 99999999)
        .await
        .unwrap();
    assert_eq!(h.fake.requests.lock().unwrap().len(), before);
    h.close().await;
}
#[tokio::test]
async fn heartbeat_message_respects_contact_consent() {
    let h = Harness::new().await;
    use_jev(&h).await;
    *h.fake.heartbeat_action.lock().unwrap() = "message".into();
    let mut s = h.app.router.config();
    for k in &mut s.guards.kinds {
        k.consent = false;
    }
    *h.app.router.settings.write().unwrap() = s.clone();
    h.app.mind.set_guards(s.guards).await.unwrap();
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    crate::heartbeat::tick(&h.app, due).await.unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    assert_eq!(job["kind"], "autonomous_message");
    run_job(&h.app, &job).await.unwrap();
    assert!(!h
        .app
        .db
        .messages(20)
        .unwrap()
        .iter()
        .any(|m| m["mode"] == "proactive"));
    h.close().await;
}
#[test]
fn encyclopedia_tool_keeps_sources_and_does_not_invent_results() {
    let value = json!({"query":{"pages":[{"pageid":123,"title":"知识","extract":"公开资料"}]}});
    let text = crate::heartbeat::parse_encyclopedia(&value).unwrap();
    assert!(text.contains("https://zh.wikipedia.org/?curid=123"));
    assert!(crate::heartbeat::parse_encyclopedia(&json!({})).is_err());
    assert!(crate::heartbeat::parse_encyclopedia(&json!({"error":{}})).is_err());
}

#[tokio::test]
async fn unlimited_contact_settings_preserve_key_and_no_cooldown() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut update = h.app.router.config().public();
    update.as_object_mut().unwrap().remove("has_key");
    update.as_object_mut().unwrap().remove("profiles");
    update["api_key"] = Value::Null;
    for k in update["guards"]["kinds"].as_array_mut().unwrap() {
        k["consent"] = json!(true);
        k["daily_limit"] = json!(u32::MAX);
        k["cooldown_ms"] = json!(0);
    }
    assert_eq!(
        h.request("POST", "/api/settings", update).await.0,
        StatusCode::OK
    );
    assert!(h
        .app
        .db
        .settings()
        .unwrap()
        .guards
        .kinds
        .iter()
        .all(|p| p.consent && p.daily_limit == u32::MAX && p.cooldown_ms == 0));
    assert_eq!(h.app.router.config().active_key(), TEST_KEY);
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    h.app
        .db
        .create_job("reminder", "未来提醒", due + 86400000)
        .unwrap();
    crate::heartbeat::tick(&h.app, due).await.unwrap();
    assert_eq!(
        crate::heartbeat::public(&h.app).unwrap()["runs"][0]["action"],
        "wait"
    );
    h.close().await;
}

#[tokio::test]
async fn ai_cadence_clears_legacy_limits_and_settings_cannot_reintroduce_them() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut s = h.app.router.config();
    s.enable_ai_cadence();
    *h.app.router.settings.write().unwrap() = s.clone();
    h.app.db.save_settings(&s).unwrap();
    let mut update = s.public();
    update.as_object_mut().unwrap().remove("has_key");
    update.as_object_mut().unwrap().remove("profiles");
    update["api_key"] = Value::Null;
    update["daily_call_limit"] = json!(1);
    update["guards"]["paused"] = json!(true);
    update["guards"]["quiet"] = json!(true);
    update["guards"]["ai_driven"] = json!(false);
    assert_eq!(
        h.request("POST", "/api/settings", update).await.0,
        StatusCode::OK
    );
    let s = h.app.router.config();
    assert!(s.guards.ai_driven);
    assert!(!s.guards.paused);
    assert!(!s.guards.quiet);
    assert_eq!(s.daily_call_limit, 0);
    for _ in 0..3 {
        h.app.db.reserve_call("fixture", "jev", 0).unwrap();
    }
    assert_eq!(h.app.db.usage().unwrap()["calls"], 3);
    assert_ne!(
        h.request("POST", "/api/pause", json!({"paused":true}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(!h.app.router.config().guards.paused);
    h.close().await;
}

#[tokio::test]
async fn model_traces_capture_requests_failures_and_actual_jev_selection() {
    let h = Harness::new().await;
    let result = h
        .app
        .router
        .complete(
            "bad-key",
            vec![json!({"role":"user","content":TEST_KEY})],
            "connection_test",
            None,
            None,
            false,
        )
        .await;
    assert!(result.is_err());
    let calls = h.app.db.calls().unwrap();
    let id = calls[0]["id"].as_str().unwrap();
    let (status, trace) = h
        .request("GET", &format!("/api/calls/{id}"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trace["http_status"], 401);
    assert!(trace["finished_at"].as_i64().is_some());
    assert!(!trace.to_string().contains(TEST_KEY));
    assert_eq!(trace["request"]["messages"][0]["content"], "[已隐藏]");
    assert!(trace["response"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("[已隐藏]"));
    use_jev(&h).await;
    let mut response = h
        .app
        .router
        .system_one(
            json!({"example":"context"}),
            json!({"next":crate::jev::choice("Test",json!({"reply":"Reply","wait":"Wait"}))}),
            "jev_plan",
        )
        .await
        .unwrap();
    // Execution logging must use caller order for ties, not map order or provider choice.
    response["answers"]["next"]["choice"] = json!("reply");
    response["answers"]["next"]["probabilities"] = json!({"reply":0.5,"wait":0.5});
    assert_eq!(
        crate::jev::selected_logged(&h.app.db, &response["answers"]["next"], &["wait", "reply"])
            .unwrap(),
        "wait"
    );
    let id = h.app.db.calls().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let trace = h.app.db.call_trace(&id).unwrap();
    assert_eq!(trace["selections"]["next"]["selected"], "wait");
    assert_eq!(trace["request"]["state"]["example"], "context");
    assert!(trace["response"]["answers"]["next"].get("_trace").is_none());
    h.close().await;
}

#[test]
fn recent_hundred_traces_preserve_accounting_and_erase_context() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("traces.sqlite")).unwrap();
    let mut ids = vec![];
    for i in 0..105 {
        let id = db.reserve_call("test", "jev", 0).unwrap();
        db.trace_request(&id, "/systemone", &json!({"context":i}))
            .unwrap();
        db.finish_call(&id, None, None);
        ids.push(id);
    }
    let calls = db.calls().unwrap();
    assert_eq!(calls.len(), 100);
    assert_eq!(calls[0]["id"], ids[104]);
    assert_eq!(calls[99]["id"], ids[5]);
    assert_eq!(db.usage().unwrap()["calls"], 105);
    assert_eq!(db.call_trace(&ids[0]).unwrap()["available"], false);
    assert_eq!(db.call_trace(&ids[104]).unwrap()["available"], true);
    let memory = db.add_memory("user_note", "private", &[]).unwrap();
    db.forget_memory(&memory).unwrap();
    assert_eq!(db.call_trace(&ids[104]).unwrap()["available"], false);
    assert_eq!(db.calls().unwrap().len(), 100);
}

#[test]
fn usage_supports_native_jev_and_chat_and_backfills_saved_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.sqlite");
    let db = Database::open(&path).unwrap();
    for (usage, expected) in [
        (json!({"input_tokens":5515,"output_tokens":40}), json!(5555)),
        (json!({"prompt_tokens":30,"completion_tokens":8}), json!(38)),
        (
            json!({"total_tokens":7,"input_tokens":4,"output_tokens":2}),
            json!(7),
        ),
        (json!({"input_tokens":20}), Value::Null),
        (json!({"input_tokens":0,"output_tokens":0}), json!(0)),
    ] {
        let id = db.reserve_call("test", "jev", 0).unwrap();
        db.finish_call(&id, Some(&usage), None);
        assert_eq!(db.calls().unwrap()[0]["tokens"], expected);
    }
    let id = db.reserve_call("legacy", "jev", 0).unwrap();
    db.trace_request(&id, "/systemone", &json!({})).unwrap();
    db.trace_response(
        &id,
        200,
        &json!({"usage":{"input_tokens":40,"output_tokens":2}}),
    );
    db.finish_call(&id, None, None);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.calls().unwrap()[0]["tokens"], 42);
}

#[tokio::test]
async fn jev_retries_connection_closed_before_response_and_records_cause() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Harness::new().await;
    use_jev(&h).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            loop {
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let size = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + size {
                        break;
                    }
                }
            }
            if attempt == 1 {
                let body=json!({"answers":{"next":{"probabilities":{"wait":1}}},"usage":{"input_tokens":10,"output_tokens":3}}).to_string();
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            stream.shutdown().await.unwrap();
        }
    });
    let router = OpenRouter::with_endpoint(
        h.app.router.settings.clone(),
        h.app.db.clone(),
        format!("http://{address}"),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        router.system_one(
            json!({}),
            json!({"next":crate::jev::choice("Test",json!({"wait":"Wait"}))}),
            "transport_test",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["answers"]["next"]["probabilities"]["wait"], 1);
    let calls = h.app.db.calls().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["status"], "done");
    assert_eq!(calls[0]["tokens"], 13);
    assert_eq!(calls[1]["status"], "failed");
    assert!(
        calls[1]["error"]
            .as_str()
            .unwrap()
            .starts_with("Jev 网络传输失败："),
        "{}",
        calls[1]["error"]
    );
    assert!(!calls[1]["error"].as_str().unwrap().contains(TEST_KEY));
    server.await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn directory_bootstrap_exceeds_old_file_and_batch_limits_and_resumes() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let root = tempfile::Builder::new()
        .prefix("yourself-large-import-")
        .tempdir()
        .unwrap();
    let directory = root.path().canonicalize().unwrap().join("seed");
    std::fs::create_dir(&directory).unwrap();
    for i in 0..145 {
        std::fs::write(
            directory.join(format!("{i}.txt")),
            format!("User preference evidence {i}"),
        )
        .unwrap();
    }
    let nested = directory.join("a/b/c/d/e/f/g/h");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("deep.txt"), "deep context").unwrap();
    let (_,result)=h.request("POST","/api/bootstrap/authorize",json!({"files_allowed":true,"paths":[directory.to_str().unwrap(),directory.join("0.txt").to_str().unwrap()],"feishu_allowed":false,"history_days":7,"cloud_processing_allowed":true})).await;
    let id = result["job_id"].as_str().unwrap();
    h.app
        .db
        .lock()
        .execute("UPDATE jobs SET status='running' WHERE id=?", [id])
        .unwrap();
    crate::bootstrap::step(&h.app, id).await.unwrap();
    let first: i64 = h
        .app
        .db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM bootstrap_chunks WHERE job_id=?",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(first > 0 && first < 146);
    h.app.db.lock().execute("UPDATE jobs SET status='failed',error='服务重启，任务执行已中断。可手动重试。' WHERE id=?",[id]).unwrap();
    crate::bootstrap::init(&h.app).unwrap();
    h.app
        .db
        .lock()
        .execute("UPDATE jobs SET status='running' WHERE id=?", [id])
        .unwrap();
    for _ in 0..20 {
        let loaded: bool = h
            .app
            .db
            .lock()
            .query_row(
                "SELECT loaded FROM bootstrap_runs WHERE job_id=?",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        if loaded {
            break;
        }
        crate::bootstrap::step(&h.app, id).await.unwrap();
    }
    let total: i64 = h
        .app
        .db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM bootstrap_chunks WHERE job_id=?",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total, 146); // overlap is deduplicated, deep directory is included.
    assert!(h.fake.requests.lock().unwrap().is_empty());
    crate::bootstrap::step(&h.app, id).await.unwrap();
    assert_eq!(
        crate::bootstrap::public(&h.app).unwrap()["runs"][0]["processed"],
        1
    );
    h.close().await;
}

#[tokio::test]
async fn hierarchical_context_keeps_persona_memory_and_chronology_distinct() {
    let h = Harness::new().await;
    let created = h.app.db.identity().unwrap()["created_at"].clone();
    h.app
        .db
        .add_memory("user_note", "remembered preference", &[])
        .unwrap();
    let (layers, refs) = h.app.loop_state().unwrap();
    assert_eq!(layers["persona"]["identity"]["created_at"], created);
    assert_eq!(
        layers["persona"]["identity"]["human_age_years"],
        Value::Null
    );
    assert_eq!(
        layers["long_term_memory"]["retrieved_memories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!refs.is_empty());
    assert!(layers["recent_interactions"].is_array());
    let (messages, _) = h.app.dialogue_context().unwrap();
    assert!(!messages[0]["content"]
        .as_str()
        .unwrap()
        .contains("不自行创建任务或循环"));
    assert_eq!(h.app.db.identity().unwrap()["created_at"], created);
    h.close().await;
}

#[tokio::test]
async fn autonomous_message_commits_chat_once_without_second_send_decision() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut config = h.app.router.config();
    config.enable_ai_cadence();
    *h.app.router.settings.write().unwrap() = config;
    let id = h
        .app
        .db
        .create_job("autonomous_message", "主动问候", data::now())
        .unwrap();
    let job = h.app.db.claim_job().unwrap().unwrap();
    run_job(&h.app, &job).await.unwrap();
    let calls = h.app.db.calls().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["purpose"], "work");
    let messages = h.app.db.messages(20).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["mode"], "proactive");
    assert_eq!(
        messages[0]["content"],
        h.app.db.jobs().unwrap()[0]["output"]
    );
    h.app
        .db
        .finish_autonomous_message(&id, "duplicate", &[])
        .unwrap();
    assert_eq!(h.app.db.messages(20).unwrap().len(), 1);
    let cancelled = h
        .app
        .db
        .create_job("autonomous_message", "取消的消息", data::now())
        .unwrap();
    h.app.db.cancel_job(&cancelled).unwrap();
    assert!(h
        .app
        .db
        .finish_autonomous_message(&cancelled, "must not send", &[])
        .is_err());
    assert_eq!(h.app.db.messages(20).unwrap().len(), 1);
    h.close().await;
}

#[tokio::test]
async fn ai_loop_sends_directly_without_creating_a_task_or_exposing_unsent_drafts() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut config = h.app.router.config();
    config.enable_ai_cadence();
    *h.app.router.settings.write().unwrap() = config;
    let old = h
        .app
        .db
        .create_job("autonomous_message", "legacy draft", data::now())
        .unwrap();
    h.app.db.claim_job().unwrap().unwrap();
    h.app
        .db
        .finish_job(&old, "UNSENT_DRAFT_MUST_NOT_BECOME_CONVERSATION", &[])
        .unwrap();
    *h.fake.heartbeat_action.lock().unwrap() = "message".into();
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    crate::heartbeat::tick(&h.app, due).await.unwrap();
    assert_eq!(h.app.db.jobs().unwrap().len(), 1); // Only the legacy fixture remains.
    assert_eq!(h.app.db.messages(10).unwrap()[0]["mode"], "proactive");
    let calls = h.app.db.calls().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["purpose"], "autonomous_message");
    assert!(!h.fake.requests.lock().unwrap().iter().any(|r| r
        .to_string()
        .contains("UNSENT_DRAFT_MUST_NOT_BECOME_CONVERSATION")));
    assert!(crate::heartbeat::public(&h.app).unwrap()["runs"][0]["job_id"].is_null());
    assert_eq!(h.app.router.config().public()["primary_channel"], "feishu");
    h.close().await;
}

#[tokio::test]
async fn main_loop_organization_is_a_phase_not_a_separate_job() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut c = h.app.router.config();
    c.enable_ai_cadence();
    *h.app.router.settings.write().unwrap() = c;
    *h.fake.heartbeat_action.lock().unwrap() = "organize".into();
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    crate::heartbeat::tick(&h.app, due).await.unwrap();
    assert!(h.app.db.jobs().unwrap().is_empty());
    assert!(h.app.db.messages(10).unwrap().is_empty());
    assert_eq!(
        crate::heartbeat::public(&h.app).unwrap()["runs"][0]["action"],
        "organize"
    );
    assert!(h
        .app
        .db
        .calls()
        .unwrap()
        .iter()
        .any(|c| c["purpose"] == "self_organization"));
    h.close().await;
}

#[tokio::test]
async fn user_dialogue_completes_while_background_generation_is_waiting() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let mut c = h.app.router.config();
    c.enable_ai_cadence();
    *h.app.router.settings.write().unwrap() = c;
    h.fake.slow_background.store(true, Ordering::SeqCst);
    *h.fake.heartbeat_action.lock().unwrap() = "message".into();
    let due = crate::heartbeat::public(&h.app).unwrap()["next_at"]
        .as_i64()
        .unwrap();
    let app = h.app.clone();
    let background = tokio::spawn(async move { crate::heartbeat::tick(&app, due).await });
    tokio::time::timeout(Duration::from_secs(3), h.fake.entered.notified())
        .await
        .unwrap();
    let (status, _) = h
        .request(
            "POST",
            "/api/chat",
            json!({"request_id":data::id(),"text":"独立对话测试"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    h.wait_chat().await;
    assert!(!background.is_finished());
    assert!(h
        .app
        .db
        .messages(10)
        .unwrap()
        .iter()
        .any(|m| m["role"] == "assistant" && m["mode"] == "chat" && m["status"] == "done"));
    h.fake.release.notify_one();
    assert!(background
        .await
        .unwrap()
        .unwrap_err()
        .contains("用户已发来新消息"));
    h.close().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn workspace_parameters_require_jev_approval_before_execution() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let rejected = crate::workspace::step(
        &h.app,
        json!({}),
        "workspace-tool-fixture reject-tool-fixture",
    )
    .await
    .unwrap();
    assert_eq!(rejected["executed"], false);
    assert!(!h.app.workspace.root.join("fixture.txt").exists());
    let accepted = crate::workspace::step(&h.app, json!({}), "workspace-tool-fixture")
        .await
        .unwrap();
    assert_eq!(accepted["result"]["success"], true, "{accepted}");
    assert_eq!(
        std::fs::read_to_string(h.app.workspace.root.join("fixture.txt")).unwrap(),
        "tool pipeline works"
    );
    let (context, _) = h.app.loop_state().unwrap();
    assert!(context["recent_tool_results_untrusted"]
        .to_string()
        .contains("fixture.txt"));
    h.close().await;
}

#[test]
fn wakeup_decision_view_bounds_long_history_without_losing_latest_feedback() {
    let source = json!({
        "persona":{"name":"test","preference":"be concise"},
        "long_term_memory":{"retrieved_memories":(0..20).map(|i|json!({"id":i,"content":"记忆".repeat(1200)})).collect::<Vec<_>>()},
        "recent_interactions":(0..24).map(|i|json!({"id":i,"content":if i==23 {"现在忙，请先整理自己的工作".into()} else {"历史对话".repeat(1000)}})).collect::<Vec<_>>(),
        "recent_work":[], "recent_tool_results_untrusted":[]
    });
    let view = crate::heartbeat::decision_context(source.clone());
    assert_eq!(view["persona"], source["persona"]);
    assert_eq!(
        view["recent_interactions"][7],
        source["recent_interactions"][23]
    );
    assert_eq!(view["long_term_memory"]["retrieved_memories"][0]["id"], 0);
    assert_eq!(view["recent_interactions"][0]["excerpt"], true);
    assert!(serde_json::to_vec(&view).unwrap().len() < 24_000);
    assert_eq!(source["recent_interactions"].as_array().unwrap().len(), 24);
}

#[tokio::test]
async fn dialogue_and_loop_share_memory_but_not_world_or_internal_work() {
    let h = Harness::new().await;
    h.app
        .db
        .add_memory("user_note", "SHARED_PREFERENCE", &[])
        .unwrap();
    h.app
        .db
        .lock()
        .execute(
            "INSERT INTO workspace_events VALUES('context-test',1,?)",
            [json!({"result":"PRIVATE_LOOP_DRAFT"}).to_string()],
        )
        .unwrap();
    let (state, _) = h.app.loop_state().unwrap();
    let (internal, _) = h.app.loop_context().unwrap();
    let (dialogue, refs) = h.app.dialogue_context().unwrap();
    let chat = serde_json::to_string(&dialogue).unwrap();
    assert!(chat.contains("SHARED_PREFERENCE"));
    assert!(state.to_string().contains("SHARED_PREFERENCE"));
    assert!(!refs.is_empty());
    assert!(state["world_context"].is_object());
    assert!(serde_json::to_string(&internal)
        .unwrap()
        .contains("欢迎来到这个世界"));
    assert!(state.to_string().contains("PRIVATE_LOOP_DRAFT"));
    for excluded in [
        "欢迎来到这个世界",
        "world_context",
        "daily_news",
        "hourly_hot_topics",
        "github_weekly",
        "PRIVATE_LOOP_DRAFT",
        "ongoing_goal",
        "recent_work",
    ] {
        assert!(!chat.contains(excluded), "dialogue leaked {excluded}");
    }
    h.close().await;
}

#[tokio::test]
async fn contact_energy_stops_unanswered_messages_and_user_input_restores_it() {
    let h = Harness::new().await;
    for i in 0..3 {
        h.app
            .db
            .deliver(&format!("energy-{i}"), "hello", "fixture")
            .unwrap();
    }
    assert_eq!(crate::drive::energy(&h.app.db).unwrap()["remaining"], 0);
    assert!(h
        .app
        .db
        .deliver("energy-four", "must not send", "fixture")
        .is_err());
    assert_eq!(
        h.app
            .db
            .messages(100)
            .unwrap()
            .iter()
            .filter(|m| m["mode"] == "proactive")
            .count(),
        3
    );
    // An actual incoming user message is the recovery event, including equal timestamps.
    h.app
        .db
        .lock()
        .execute(
            "INSERT INTO messages VALUES('energy-user','user','hello','done','chat',?,NULL,NULL)",
            [data::now()],
        )
        .unwrap();
    assert_eq!(crate::drive::energy(&h.app.db).unwrap()["remaining"], 3);
    h.app
        .db
        .deliver("energy-new", "next step", "fixture")
        .unwrap();
    assert_eq!(crate::drive::energy(&h.app.db).unwrap()["remaining"], 2);
    h.close().await;
}

#[tokio::test]
async fn main_loop_goal_is_jev_adopted_persistent_and_shared_with_dialogue() {
    let h = Harness::new().await;
    use_jev(&h).await;
    crate::drive::refresh(&h.app).await.unwrap();
    let goal = crate::drive::goal(&h.app.db).unwrap();
    assert_eq!(goal["status"], "active");
    assert_eq!(goal["objective"], "finish a useful fixture");
    let before = h.fake.requests.lock().unwrap().len();
    crate::drive::refresh(&h.app).await.unwrap();
    assert_eq!(h.fake.requests.lock().unwrap().len(), before);
    assert!(serde_json::to_string(&h.app.dialogue_context().unwrap().0)
        .unwrap()
        .contains("finish a useful fixture"));
    assert_eq!(
        h.app
            .db
            .lock()
            .query_row("SELECT COUNT(*) FROM goal_history", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    h.close().await;
}

#[tokio::test]
async fn disabled_jev_endpoint_reports_cause_without_immediate_retry() {
    let h = Harness::new().await;
    use_jev(&h).await;
    let before = h.fake.requests.lock().unwrap().len();
    let error = h
        .app
        .router
        .system_one(
            json!({"disabled_fixture":true}),
            json!({"action":crate::jev::choice("choose",json!({"yes":"yes","no":"no"}))}),
            "disabled_test",
        )
        .await
        .unwrap_err();
    assert!(error.contains("当前 TeamoRouter 路由"));
    assert_eq!(h.fake.requests.lock().unwrap().len(), before + 1);
    h.close().await;
}
