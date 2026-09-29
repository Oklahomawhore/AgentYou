use fs2::FileExt;
use mind_runtime::{Clock, Guards, SystemClock};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

pub type AppResult<T> = Result<T, String>;
pub fn now() -> i64 {
    SystemClock.now_ms()
}
pub fn id() -> String {
    Uuid::new_v4().to_string()
}
pub fn short(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Openrouter,
    Teamorouter,
}
impl Provider {
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Openrouter => "https://openrouter.ai/api/v1",
            Self::Teamorouter => "https://api.teamorouter.com/v1",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TeamoConfig {
    pub api_key: String,
    pub model: String,
    pub decision_model: String,
}
impl Default for TeamoConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "deepseek-flash".into(),
            decision_model: "jev".into(),
        }
    }
}
/// Local chat is always a mirror; this selects the proactive external adapter.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryChannel {
    Local,
    #[default]
    Feishu,
    Weixin,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub primary_channel: PrimaryChannel,
    pub provider: Provider,
    pub teamorouter: TeamoConfig,
    pub api_key: String,
    pub model: String,
    pub decision_model: String,
    pub persona_notes: String,
    pub max_tokens: u32,
    pub daily_call_limit: u32,
    pub web_search: bool,
    pub exploration_goal: String,
    pub exploration_interval_minutes: u32,
    pub guards: Guards,
}
impl Default for Settings {
    fn default() -> Self {
        Self { primary_channel: PrimaryChannel::default(), provider: Provider::default(), teamorouter: TeamoConfig::default(), api_key: String::new(), model: "openai/gpt-4.1-mini".into(), decision_model: "openai/gpt-4.1-mini".into(),
            persona_notes: "你是知微，一个原创数字角色。好奇、坦诚，重视求证，尊重对方的注意力。用自然中文交流。".into(),
            max_tokens: 2048, daily_call_limit: 80, web_search: false, exploration_goal: String::new(), exploration_interval_minutes: 0,
            guards: Guards { cloud_allowed: true, ..Guards::default() } }
    }
}
impl Settings {
    pub fn enable_ai_cadence(&mut self) {
        self.guards.ai_driven = true;
        self.guards.paused = false;
        self.guards.quiet = false;
        self.guards.busy = false;
        self.daily_call_limit = 0;
        for kind in &mut self.guards.kinds {
            kind.consent = true;
            kind.daily_limit = u32::MAX;
            kind.cooldown_ms = 0;
        }
    }

    pub fn uses_jev(&self) -> bool {
        self.provider == Provider::Teamorouter && self.active_decision_model() == "jev"
    }

    pub fn active_key(&self) -> &str {
        match self.provider {
            Provider::Openrouter => &self.api_key,
            Provider::Teamorouter => &self.teamorouter.api_key,
        }
    }
    pub fn active_model(&self) -> &str {
        match self.provider {
            Provider::Openrouter => &self.model,
            Provider::Teamorouter => &self.teamorouter.model,
        }
    }
    pub fn active_decision_model(&self) -> &str {
        match self.provider {
            Provider::Openrouter => &self.decision_model,
            Provider::Teamorouter => &self.teamorouter.decision_model,
        }
    }

    pub fn public(&self) -> Value {
        json!({"primary_channel":self.primary_channel,"provider":self.provider,"profiles":{
            "openrouter":{"has_key":!self.api_key.is_empty(),"model":self.model,"decision_model":self.decision_model},
            "teamorouter":{"has_key":!self.teamorouter.api_key.is_empty(),"model":self.teamorouter.model,"decision_model":self.teamorouter.decision_model}},
            "has_key": !self.active_key().is_empty(), "model": self.active_model(), "decision_model": self.active_decision_model(),
            "persona_notes": self.persona_notes,"max_tokens":self.max_tokens,"daily_call_limit":self.daily_call_limit,
            "web_search":self.web_search,"exploration_goal":self.exploration_goal,"exploration_interval_minutes":self.exploration_interval_minutes,"guards":self.guards})
    }
    pub fn validate(&self) -> AppResult<()> {
        if self.active_model() == "jev"
            || (self.active_decision_model() == "jev" && self.provider != Provider::Teamorouter)
        {
            return Err(
                "Jev 仅可用作 TeamoRouter 判定模型；对话请选择 DeepSeek 等聊天模型。".into(),
            );
        }

        if [&self.api_key, &self.teamorouter.api_key]
            .iter()
            .any(|key| key.len() > 512 || key.chars().any(char::is_whitespace))
        {
            return Err("API Key 格式不正确，请检查空格和换行。".into());
        }
        for model in [
            &self.model,
            &self.decision_model,
            &self.teamorouter.model,
            &self.teamorouter.decision_model,
        ] {
            if model.is_empty() || model.len() > 160 || model.chars().any(char::is_whitespace) {
                return Err("请填写有效的模型 ID。".into());
            }
        }
        if !(128..=8192).contains(&self.max_tokens)
            || (self.daily_call_limit != 0 && !(1..=2000).contains(&self.daily_call_limit))
            || self.persona_notes.len() > 8000
        {
            return Err(
                "输出上限需为 128–8192，每日调用上限需为 1–2000，人格说明请控制在 8000 字节内。"
                    .into(),
            );
        }
        if self.exploration_goal.len() > 4000
            || (self.exploration_interval_minutes != 0
                && (!(30..=43200).contains(&self.exploration_interval_minutes)
                    || self.exploration_goal.trim().is_empty()))
        {
            return Err("持续探索需要填写目标，间隔至少 30 分钟；设为 0 表示关闭。".into());
        }
        self.guards.validate().map_err(err)
    }
}

pub struct Database {
    db: Mutex<Connection>,
    _lock: File,
}
impl Database {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(err)?;
        lock.try_lock_exclusive()
            .map_err(|_| "已有一个服务正在使用该数据目录。".to_string())?;
        let db = Connection::open(path).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, role TEXT NOT NULL, content TEXT NOT NULL,
              status TEXT NOT NULL, mode TEXT NOT NULL, created_at INTEGER NOT NULL, error TEXT, reply_to TEXT);
            CREATE TABLE IF NOT EXISTS memories(id TEXT PRIMARY KEY, kind TEXT NOT NULL, content TEXT NOT NULL, created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, kind TEXT NOT NULL, objective TEXT NOT NULL, status TEXT NOT NULL,
              due_at INTEGER NOT NULL, created_at INTEGER NOT NULL, output TEXT NOT NULL DEFAULT '', error TEXT);
            CREATE TABLE IF NOT EXISTS dependencies(owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, source_id TEXT NOT NULL,
              PRIMARY KEY(owner_type,owner_id,source_id));
            CREATE TABLE IF NOT EXISTS workspace_events(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS attention(id INTEGER PRIMARY KEY CHECK(id=1),payload TEXT NOT NULL);
            INSERT OR IGNORE INTO attention VALUES(1,'{}');
            CREATE TABLE IF NOT EXISTS learned_skills(name TEXT NOT NULL,version INTEGER NOT NULL,created_at INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(name,version));
            CREATE TABLE IF NOT EXISTS agent_drive(id INTEGER PRIMARY KEY CHECK(id=1),payload TEXT NOT NULL,reviewed_event TEXT NOT NULL,attempted_at INTEGER NOT NULL DEFAULT 0);
            INSERT OR IGNORE INTO agent_drive VALUES(1,'{}','',0);
            CREATE TABLE IF NOT EXISTS goal_history(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,action TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS agent_identity(id INTEGER PRIMARY KEY CHECK(id=1),created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS calls(id TEXT PRIMARY KEY, purpose TEXT NOT NULL, model TEXT NOT NULL,
              status TEXT NOT NULL, created_at INTEGER NOT NULL, tokens INTEGER, cost REAL, error TEXT);
            CREATE TABLE IF NOT EXISTS call_traces(call_id TEXT PRIMARY KEY, request TEXT NOT NULL, response TEXT, endpoint TEXT NOT NULL, http_status INTEGER, finished_at INTEGER, selections TEXT NOT NULL DEFAULT '{}');
            CREATE TABLE IF NOT EXISTS jev_plans(id TEXT PRIMARY KEY, source_id TEXT NOT NULL, phase TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS notification_kinds(message_id TEXT PRIMARY KEY, kind TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memory_revisions(old_id TEXT NOT NULL,new_id TEXT NOT NULL,PRIMARY KEY(old_id,new_id));
            CREATE TABLE IF NOT EXISTS memory_provenance(memory_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, fragment INTEGER NOT NULL, origin TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(source_id,fragment));
            CREATE TABLE IF NOT EXISTS pending_erasure(id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS autonomy(id INTEGER PRIMARY KEY CHECK(id=1), last_scheduled INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS outbox(id TEXT PRIMARY KEY, event_id TEXT UNIQUE NOT NULL, status TEXT NOT NULL,
              message_id TEXT NOT NULL, created_at INTEGER NOT NULL);
            UPDATE messages SET status='failed',error='服务重启，中断的回复未自动重试。' WHERE status='pending';
            UPDATE jobs SET status='failed',error='服务重启，任务执行已中断。可手动重试。' WHERE status='running';
            UPDATE calls SET status='unknown',error='服务重启，请求计费状态未知。' WHERE status='running';
        ").map_err(err)?;
        db.execute("INSERT OR IGNORE INTO agent_identity VALUES(1,COALESCE((SELECT MIN(created_at) FROM messages),?))",[now()]).map_err(err)?;
        // Recover totals only where an original usage receipt is available.
        {
            let mut stmt=db.prepare("SELECT c.id,t.response FROM calls c JOIN call_traces t ON t.call_id=c.id WHERE c.tokens IS NULL AND t.response IS NOT NULL").map_err(err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(err)?;
            for row in rows {
                let (id, raw) = row.map_err(err)?;
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    if let Some(tokens) = v.get("usage").and_then(usage_tokens) {
                        db.execute("UPDATE calls SET tokens=? WHERE id=?", params![tokens, id])
                            .map_err(err)?;
                    }
                }
            }
        }
        Ok(Self {
            db: Mutex::new(db),
            _lock: lock,
        })
    }
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn identity(&self) -> AppResult<Value> {
        let created: i64 = self
            .lock()
            .query_row(
                "SELECT created_at FROM agent_identity WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        Ok(
            json!({"kind":"original","created_at":created,"existence_seconds":(now()-created).max(0)/1000,"human_age_years":null,"development":"Starts without autobiographical experience; user understanding and self preferences develop from sourced interaction, not elapsed time alone. Base model knowledge is not personal experience."}),
        )
    }
    pub fn settings(&self) -> AppResult<Settings> {
        let raw: Option<String> = self
            .lock()
            .query_row("SELECT payload FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        raw.map(|s| serde_json::from_str(&s).map_err(err))
            .unwrap_or_else(|| Ok(Settings::default()))
    }
    pub fn save_settings(&self, s: &Settings) -> AppResult<()> {
        s.validate()?;
        self.lock().execute("INSERT INTO settings VALUES (1,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", [serde_json::to_string(s).map_err(err)?]).map_err(err)?;
        Ok(())
    }
    pub fn messages(&self, limit: u32) -> AppResult<Vec<Value>> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT id,role,content,status,mode,created_at,error,reply_to FROM (SELECT rowid,* FROM messages ORDER BY rowid DESC LIMIT ?) ORDER BY rowid").map_err(err)?;
        let rows=stmt.query_map([limit], |r|Ok(json!({"id":r.get::<_,String>(0)?,"role":r.get::<_,String>(1)?,"content":r.get::<_,String>(2)?,
            "status":r.get::<_,String>(3)?,"mode":r.get::<_,String>(4)?,"created_at":r.get::<_,i64>(5)?,"error":r.get::<_,Option<String>>(6)?,"reply_to":r.get::<_,Option<String>>(7)?}))).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
    pub fn memories(&self) -> AppResult<Vec<Value>> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT m.id,m.kind,m.content,m.created_at,p.origin,EXISTS(SELECT 1 FROM memory_revisions r JOIN memories n ON n.id=r.new_id WHERE r.old_id=m.id) FROM memories m LEFT JOIN memory_provenance p ON p.memory_id=m.id ORDER BY m.created_at DESC LIMIT 200").map_err(err)?;
        let rows=stmt.query_map([], |r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"content":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,"source":r.get::<_,Option<String>>(4)?,"superseded":r.get::<_,bool>(5)?}))).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
    pub fn jobs(&self) -> AppResult<Vec<Value>> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT id,kind,objective,status,due_at,created_at,output,error FROM jobs ORDER BY created_at DESC LIMIT 100").map_err(err)?;
        let rows=stmt.query_map([], |r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"objective":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,
            "due_at":r.get::<_,i64>(4)?,"created_at":r.get::<_,i64>(5)?,"output":r.get::<_,String>(6)?,"error":r.get::<_,Option<String>>(7)?}))).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
    pub fn begin_chat(&self, request_id: &str, text: &str) -> AppResult<Option<String>> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        if tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?)",
                [request_id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(err)?
        {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO messages VALUES (?,'user',?,'done','chat',?,NULL,NULL)",
            params![request_id, text, now()],
        )
        .map_err(err)?;
        let reply = id();
        tx.execute(
            "INSERT INTO messages VALUES (?,'assistant','','pending','chat',?,NULL,?)",
            params![reply, now(), request_id],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO dependencies VALUES ('message',?,?)",
            params![reply, request_id],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Some(reply))
    }
    pub fn complete_chat(&self, message: &str, content: &str, sources: &[String]) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let changed=tx.execute("UPDATE messages SET content=?,status='done',error=NULL WHERE id=? AND status='pending'",params![content,message]).map_err(err)?;
        if changed != 1 {
            return Err("回复已经取消。".into());
        }
        for source in sources {
            tx.execute(
                "INSERT OR IGNORE INTO dependencies VALUES ('message',?,?)",
                params![message, source],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    pub fn save_plan(&self, source: &str, phase: &str, plan: &crate::jev::Plan) -> AppResult<()> {
        self.lock()
            .execute(
                "INSERT INTO jev_plans VALUES(?,?,?,?,?)",
                params![
                    id(),
                    source,
                    phase,
                    serde_json::to_string(plan).map_err(err)?,
                    now()
                ],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn plans(&self) -> AppResult<Vec<Value>> {
        let db = self.lock();
        let mut statement = db
            .prepare(
                "SELECT phase,payload,created_at FROM jev_plans ORDER BY created_at DESC LIMIT 60",
            )
            .map_err(err)?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(err)?;
        rows.map(|row|{let (phase,payload,created_at)=row.map_err(err)?;Ok(json!({"phase":phase,"plan":serde_json::from_str::<Value>(&payload).map_err(err)?,"created_at":created_at}))}).collect()
    }
    pub fn skip_chat(&self, message: &str, sources: &[String]) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        if tx.execute("UPDATE messages SET status='skipped',content='Jev 判定当前无需回复。',error=NULL WHERE id=? AND status='pending'",[message]).map_err(err)?!=1 {return Err("回复已经取消。".into());}
        for source in sources {
            tx.execute(
                "INSERT OR IGNORE INTO dependencies VALUES('message',?,?)",
                params![message, source],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    pub fn fail_chat(&self, message: &str, error: &str) {
        let _ = self.lock().execute(
            "UPDATE messages SET status='failed',error=? WHERE id=? AND status='pending'",
            params![error, message],
        );
    }
    pub fn add_memory(&self, kind: &str, content: &str, sources: &[String]) -> AppResult<String> {
        if content.trim().is_empty() || content.len() > 16000 {
            return Err("记忆内容不能为空，且不超过 16000 字节。".into());
        }
        if ![
            "user_note",
            "reflection",
            "source_knowledge",
            "belief",
            "plan",
        ]
        .contains(&kind)
        {
            return Err("未知记忆类型。".into());
        }
        let key = id();
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        tx.execute(
            "INSERT INTO memories VALUES (?,?,?,?)",
            params![key, kind, content, now()],
        )
        .map_err(err)?;
        for source in sources {
            tx.execute(
                "INSERT OR IGNORE INTO dependencies VALUES ('memory',?,?)",
                params![key, source],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(key)
    }
    pub fn forget_memory(&self, key: &str) -> AppResult<Vec<String>> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memories WHERE id=?)",
                [key],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Err("记忆不存在。".into());
        }
        let mut queue = vec![("memory".to_string(), key.to_string())];
        let mut visited = std::collections::HashSet::new();
        while let Some((kind, key)) = queue.pop() {
            if !visited.insert(key.clone()) {
                continue;
            }
            let children: Vec<(String, String)> = {
                let mut stmt = tx
                    .prepare("SELECT owner_type,owner_id FROM dependencies WHERE source_id=?")
                    .map_err(err)?;
                let rows = stmt
                    .query_map([&key], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(err)?;
                rows.collect::<Result<_, _>>().map_err(err)?
            };
            queue.extend(children);
            tx.execute("INSERT OR IGNORE INTO pending_erasure VALUES (?)", [&key])
                .map_err(err)?;
            match kind.as_str() {
                "memory" => {
                    tx.execute("DELETE FROM memories WHERE id=?", [&key])
                        .map_err(err)?;
                }
                "message" => {
                    tx.execute("DELETE FROM messages WHERE id=?", [&key])
                        .map_err(err)?;
                    if tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='feishu_stream')",
                            [],
                            |r| r.get::<_, bool>(0),
                        )
                        .map_err(err)?
                    {
                        tx.execute("DELETE FROM feishu_stream WHERE message_id=?", [&key])
                            .map_err(err)?;
                    }
                    tx.execute("DELETE FROM outbox WHERE message_id=?", [&key])
                        .map_err(err)?;
                }
                "goal" => {
                    tx.execute("UPDATE execution_goals SET status='cancelled',objective='来源已删除',transcript='[]',output='',error='依赖记忆已删除' WHERE id=?",[&key]).map_err(err)?;
                    tx.execute("DELETE FROM execution_steps WHERE goal_id=?", [&key])
                        .map_err(err)?;
                }
                "job" => {
                    tx.execute("UPDATE jobs SET status='cancelled',objective='来源已删除',output='',error='依赖记忆已删除。' WHERE id=?",[&key]).map_err(err)?;
                }
                _ => {}
            }
            tx.execute(
                "DELETE FROM dependencies WHERE owner_id=?1 OR source_id=?1",
                [&key],
            )
            .map_err(err)?;
        }
        if tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='decision_paths')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(err)?
        {
            tx.execute("DELETE FROM decision_paths", []).map_err(err)?;
            tx.execute("UPDATE decision_preferences SET payload='{\"message\":50,\"continue_work\":50,\"new_work\":50,\"no_action\":50}',revision='' WHERE id=1",[]).map_err(err)?;
        }
        tx.execute("DELETE FROM call_traces", []).map_err(err)?;
        tx.execute("DELETE FROM learned_skills", []).map_err(err)?;
        tx.execute("UPDATE attention SET payload='{}' WHERE id=1", [])
            .map_err(err)?;
        tx.execute("DELETE FROM goal_history", []).map_err(err)?;
        tx.execute(
            "UPDATE agent_drive SET payload='{}',reviewed_event='',attempted_at=0",
            [],
        )
        .map_err(err)?;
        tx.execute("DELETE FROM workspace_events", [])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(visited.into_iter().collect())
    }
    pub fn pending_erasure(&self) -> AppResult<Vec<String>> {
        let db = self.lock();
        let mut stmt = db.prepare("SELECT id FROM pending_erasure").map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)
    }
    pub fn finish_erasure(&self, ids: &[String]) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        for id in ids {
            tx.execute("DELETE FROM pending_erasure WHERE id=?", [id])
                .map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    pub fn create_job(&self, kind: &str, objective: &str, due_at: i64) -> AppResult<String> {
        if ![
            "task",
            "research",
            "reflection",
            "reminder",
            "curiosity",
            "check_in",
            "self_review",
            "autonomous_message",
            "knowledge_search",
        ]
        .contains(&kind)
            || objective.trim().is_empty()
            || objective.len() > 12000
            || due_at < 0
        {
            return Err("任务类型、内容或时间无效。".into());
        }
        let key = id();
        let db = self.lock();
        let count: u32 = db
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE status IN ('queued','running')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if count >= 30 {
            return Err("最多同时保留 30 个待执行任务。".into());
        }
        db.execute("INSERT INTO jobs(id,kind,objective,status,due_at,created_at) VALUES (?,?,?,'queued',?,?)",params![key,kind,objective,due_at,now()]).map_err(err)?;
        Ok(key)
    }
    pub fn schedule_exploration(&self, settings: &Settings) -> AppResult<()> {
        if settings.exploration_interval_minutes == 0 {
            return Ok(());
        }
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let last: Option<i64> = tx
            .query_row("SELECT last_scheduled FROM autonomy WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(err)?;
        if last.is_some_and(|t| {
            now().saturating_sub(t) < i64::from(settings.exploration_interval_minutes) * 60_000
        }) {
            return Ok(());
        }
        let active: u32 = tx
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE status IN ('queued','running')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if active > 0 {
            return Ok(());
        }
        tx.execute("INSERT INTO jobs(id,kind,objective,status,due_at,created_at) VALUES (?,'research',?,'queued',?,?)",params![id(),settings.exploration_goal,now(),now()]).map_err(err)?;
        tx.execute("INSERT INTO autonomy VALUES (1,?) ON CONFLICT(id) DO UPDATE SET last_scheduled=excluded.last_scheduled",[now()]).map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn claim_job(&self) -> AppResult<Option<Value>> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let row:Option<Value>=tx.query_row("SELECT id,kind,objective FROM jobs WHERE status='queued' AND kind!='cold_start' AND due_at<=? ORDER BY due_at,created_at LIMIT 1",[now()],
            |r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"objective":r.get::<_,String>(2)?}))).optional().map_err(err)?;
        if let Some(job) = &row {
            tx.execute(
                "UPDATE jobs SET status='running' WHERE id=?",
                [job["id"].as_str().unwrap()],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(row)
    }
    pub fn job_running(&self, key: &str) -> bool {
        self.lock()
            .query_row("SELECT status='running' FROM jobs WHERE id=?", [key], |r| {
                r.get(0)
            })
            .unwrap_or(false)
    }
    pub fn finish_job(&self, key: &str, output: &str, sources: &[String]) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        if tx
            .execute(
                "UPDATE jobs SET status='done',output=?,error=NULL WHERE id=? AND status='running'",
                params![output, key],
            )
            .map_err(err)?
            != 1
        {
            return Err("任务已取消。".into());
        }
        for source in sources {
            tx.execute(
                "INSERT OR IGNORE INTO dependencies VALUES ('job',?,?)",
                params![key, source],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }
    pub fn finish_autonomous_message(
        &self,
        key: &str,
        output: &str,
        sources: &[String],
    ) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let event = format!("job-{key}");
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox WHERE event_id=?)",
                [&event],
                |r| r.get(0),
            )
            .map_err(err)?;
        if exists {
            return Ok(());
        }
        if tx.execute("UPDATE jobs SET status='done',output=?,error=NULL WHERE id=? AND kind='autonomous_message' AND status='running'",params![output,key]).map_err(err)?!=1 {return Err("主动消息任务已取消或不在执行中。".into());}
        for source in sources {
            tx.execute(
                "INSERT OR IGNORE INTO dependencies VALUES ('job',?,?)",
                params![key, source],
            )
            .map_err(err)?;
        }
        let message = id();
        tx.execute(
            "INSERT INTO messages VALUES (?,'assistant',?,'done','proactive',?,NULL,NULL)",
            params![message, output, now()],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO dependencies VALUES ('message',?,?)",
            params![message, key],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO outbox VALUES (?,?,'SENT',?,?)",
            params![id(), event, message, now()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn fail_job(&self, key: &str, error: &str) {
        let _ = self.lock().execute(
            "UPDATE jobs SET status='failed',error=? WHERE id=? AND status='running'",
            params![error, key],
        );
    }
    pub fn cancel_job(&self, key: &str) -> AppResult<()> {
        if self.lock().execute("UPDATE jobs SET status='cancelled',error='用户已取消。' WHERE id=? AND status IN ('running','queued')",[key]).map_err(err)?==0 {return Err("该任务已经结束或不存在。".into());}
        Ok(())
    }
    pub fn retry_job(&self, key: &str) -> AppResult<()> {
        if self.lock().execute("UPDATE jobs SET status='queued',error=NULL,due_at=? WHERE id=? AND status IN ('failed','cancelled')",params![now(),key]).map_err(err)?==0 {return Err("只能重试失败或已取消的任务。".into());}
        Ok(())
    }
    pub fn reserve_call(&self, purpose: &str, model: &str, limit: u32) -> AppResult<String> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let count: u32 = tx
            .query_row(
                "SELECT COUNT(*) FROM calls WHERE created_at>?",
                [now() - 86_400_000],
                |r| r.get(0),
            )
            .map_err(err)?;
        if limit != 0 && count >= limit {
            return Err("已达到过去 24 小时调用上限，可在设置中调整。".into());
        }
        let key = id();
        tx.execute(
            "INSERT INTO calls(id,purpose,model,status,created_at) VALUES (?,?,?,'running',?)",
            params![key, purpose, model, now()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(key)
    }
    pub fn trace_request(&self, key: &str, endpoint: &str, request: &Value) -> AppResult<()> {
        let db = self.lock();
        db.execute(
            "INSERT INTO call_traces(call_id,endpoint,request) VALUES (?,?,?)",
            params![key, endpoint, request.to_string()],
        )
        .map_err(err)?;
        db.execute("DELETE FROM call_traces WHERE call_id NOT IN (SELECT id FROM calls ORDER BY rowid DESC LIMIT 100)", []).map_err(err)?;
        Ok(())
    }
    pub fn trace_response(&self, key: &str, status: u16, response: &Value) {
        let _ = self.lock().execute(
            "UPDATE call_traces SET response=?,http_status=? WHERE call_id=?",
            params![response.to_string(), status, key],
        );
    }
    pub fn trace_selection(&self, key: &str, question: &str, selection: Value) {
        let db = self.lock();
        let Ok(raw) = db.query_row(
            "SELECT selections FROM call_traces WHERE call_id=?",
            [key],
            |r| r.get::<_, String>(0),
        ) else {
            return;
        };
        let mut values: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
        values[question] = selection;
        let _ = db.execute(
            "UPDATE call_traces SET selections=? WHERE call_id=?",
            params![values.to_string(), key],
        );
    }
    pub fn call_trace(&self, key: &str) -> AppResult<Value> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT request,response,endpoint,http_status,finished_at,selections FROM call_traces WHERE call_id=?").map_err(err)?;
        let mut rows = stmt.query([key]).map_err(err)?;
        let Some(r) = rows.next().map_err(err)? else {
            return Ok(json!({"available":false}));
        };
        let parse = |s: Option<String>| {
            s.and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null)
        };
        Ok(
            json!({"available":true,"request":parse(r.get(0).map_err(err)?),"response":parse(r.get(1).map_err(err)?),"endpoint":r.get::<_,String>(2).map_err(err)?,"http_status":r.get::<_,Option<u16>>(3).map_err(err)?,"finished_at":r.get::<_,Option<i64>>(4).map_err(err)?,"selections":parse(r.get(5).map_err(err)?)}),
        )
    }
    pub fn finish_call(&self, key: &str, usage: Option<&Value>, error: Option<&str>) {
        let _ = self.lock().execute(
            "UPDATE call_traces SET finished_at=? WHERE call_id=?",
            params![now(), key],
        );
        let _ = self.lock().execute(
            "UPDATE calls SET status=?,tokens=?,cost=?,error=? WHERE id=?",
            params![
                if error.is_some() { "failed" } else { "done" },
                usage.and_then(usage_tokens),
                usage.and_then(|u| u["cost"].as_f64()),
                error,
                key
            ],
        );
    }
    pub fn uncertain_call(&self, key: &str) {
        let _=self.lock().execute("UPDATE calls SET status='unknown',error='本地请求已中止，远端计费状态未知。' WHERE id=? AND status='running'",[key]);
    }
    pub fn usage(&self) -> AppResult<Value> {
        self.lock().query_row("SELECT COUNT(*),COALESCE(SUM(tokens),0),SUM(cost),SUM(CASE WHEN status='done' AND cost IS NULL THEN 1 ELSE 0 END) FROM calls WHERE created_at>?",[now()-86_400_000],
            |r|Ok(json!({"calls":r.get::<_,u32>(0)?,"tokens":r.get::<_,i64>(1)?,"cost":r.get::<_,Option<f64>>(2)?,"unpriced":r.get::<_,Option<u32>>(3)?}))).map_err(err)
    }
    pub fn calls(&self) -> AppResult<Vec<Value>> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT purpose,model,status,created_at,tokens,cost,error,id FROM calls ORDER BY rowid DESC LIMIT 100").map_err(err)?;
        let rows=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(7)?,"purpose":r.get::<_,String>(0)?,"model":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,
            "tokens":r.get::<_,Option<i64>>(4)?,"cost":r.get::<_,Option<f64>>(5)?,"error":r.get::<_,Option<String>>(6)?}))).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
    // Local UI is the only delivery sink: message and receipt commit atomically.
    pub fn deliver(&self, event: &str, content: &str, source: &str) -> AppResult<()> {
        self.deliver_typed(event, content, source, None)
    }
    pub fn deliver_typed(
        &self,
        event: &str,
        content: &str,
        source: &str,
        kind: Option<mind_runtime::InitiativeKind>,
    ) -> AppResult<()> {
        let mut db = self.lock();
        let tx = db.transaction().map_err(err)?;
        let msg = id();
        let out = id();
        let reserved = tx
            .execute(
                "INSERT OR IGNORE INTO outbox VALUES (?,?,'READY',?,?)",
                params![out, event, msg, now()],
            )
            .map_err(err)?;
        if reserved == 0 {
            return Ok(());
        }
        tx.execute("UPDATE outbox SET status='SENDING' WHERE id=?", [&out])
            .map_err(err)?;
        tx.execute(
            "INSERT INTO messages VALUES (?,'assistant',?,'done','proactive',?,NULL,NULL)",
            params![msg, content, now()],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO dependencies VALUES ('message',?,?)",
            params![msg, source],
        )
        .map_err(err)?;
        if let Some(kind) = kind {
            tx.execute(
                "INSERT INTO notification_kinds VALUES(?,?)",
                params![
                    msg,
                    serde_json::to_value(kind).map_err(err)?.as_str().unwrap()
                ],
            )
            .map_err(err)?;
        }
        tx.execute("UPDATE outbox SET status='SENT' WHERE id=?", [&out])
            .map_err(err)?;
        tx.commit().map_err(err)
    }
}

fn usage_tokens(usage: &Value) -> Option<i64> {
    let count = |key: &str| usage[key].as_i64().filter(|n| *n >= 0);
    count("total_tokens").or_else(|| {
        let input = count("input_tokens").or_else(|| count("prompt_tokens"))?;
        let output = count("output_tokens").or_else(|| count("completion_tokens"))?;
        input.checked_add(output)
    })
}
