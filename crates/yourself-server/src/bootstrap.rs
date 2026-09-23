//! Explicit, bounded cold start. No files/history read until POST /bootstrap/authorize.
use crate::{
    data::{self, AppResult},
    feishu,
    memory::{self, Item},
    service::{self, App},
};
use axum::{extract::State, response::Response, Json};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::Path,
    sync::{atomic::Ordering, Arc},
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub files_allowed: bool,
    pub paths: Vec<String>,
    pub feishu_allowed: bool,
    pub history_days: u32,
    pub cloud_processing_allowed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Scope {
    request: Request,
    feishu: Option<feishu::Config>,
}
pub fn init(app: &App) -> AppResult<()> {
    app.db.lock().execute_batch("CREATE TABLE IF NOT EXISTS bootstrap_prompt(id INTEGER PRIMARY KEY CHECK(id=1),choice TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS bootstrap_runs(job_id TEXT PRIMARY KEY,scope TEXT NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,loaded INTEGER NOT NULL DEFAULT 0,report TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS bootstrap_paths(job_id TEXT NOT NULL,path TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'pending',note TEXT NOT NULL DEFAULT '',PRIMARY KEY(job_id,path));
        CREATE TABLE IF NOT EXISTS bootstrap_chunks(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,origin TEXT NOT NULL,body TEXT NOT NULL,decision TEXT);
        UPDATE jobs SET status='queued',error=NULL WHERE kind='cold_start' AND status='failed' AND error='服务重启，任务执行已中断。可手动重试。';").map_err(err)
}
pub fn public(app: &App) -> AppResult<Value> {
    let db = app.db.lock();
    let choice: Option<String> = db
        .query_row("SELECT choice FROM bootstrap_prompt WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    let mut q=db.prepare("SELECT j.id,j.status,j.output,j.error,r.revoked,r.report,(SELECT COUNT(*) FROM bootstrap_chunks c WHERE c.job_id=j.id),(SELECT COUNT(*) FROM bootstrap_chunks c WHERE c.job_id=j.id AND (c.decision IS NOT NULL OR j.status='done')) FROM jobs j JOIN bootstrap_runs r ON r.job_id=j.id ORDER BY j.created_at DESC LIMIT 10").map_err(err)?;
    let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"output":r.get::<_,String>(2)?,"error":r.get::<_,Option<String>>(3)?,"revoked":r.get::<_,bool>(4)?,"report":r.get::<_,String>(5)?,"total":r.get::<_,i64>(6)?,"processed":r.get::<_,i64>(7)?}))).map_err(err)?;
    Ok(
        json!({"needs_prompt":choice.is_none(),"runs":rows.collect::<Result<Vec<_>,_>>().map_err(err)?}),
    )
}
pub async fn get(State(app): State<Arc<App>>) -> Response {
    service::api(public(&app))
}
pub async fn dismiss(State(app): State<Arc<App>>) -> Response {
    service::api(app.db.lock().execute("INSERT INTO bootstrap_prompt VALUES(1,'later') ON CONFLICT(id) DO UPDATE SET choice='later'",[]).map(|_|json!({"ok":true})).map_err(err))
}
pub async fn authorize(State(app): State<Arc<App>>, Json(request): Json<Request>) -> Response {
    service::api(async {
        let _guard=app.control.lock().await;
        if !request.cloud_processing_allowed || (!request.files_allowed && !request.feishu_allowed){return Err("请选择资料来源，并明确允许将选定资料发送给 Jev 处理。".into());}
        if ![7,30,90].contains(&request.history_days) || request.paths.iter().any(|p|p.len()>4096){return Err("历史范围或路径长度无效。".into());}
        if request.files_allowed && request.paths.is_empty(){return Err("请选择文件或填写具体目录。".into());}
        let active:bool=app.db.lock().query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='cold_start' AND status IN ('queued','running'))",[],|r|r.get(0)).map_err(err)?;
        if active{return Err("已有冷启动任务，请等待或先撤销。".into());}
        let mut request=request;
        if !request.files_allowed{request.paths.clear();}
        // Metadata validation only, after consent. Never silently expand a root to home/all disks.
        for path in &mut request.paths {let p=Path::new(path);if !p.is_absolute(){return Err("请输入绝对路径。".into());}validate_path(p)?;let canonical=p.canonicalize().map_err(|_|"文件或目录不存在/无访问权限")?;if canonical==Path::new("/") || std::env::var_os("HOME").is_some_and(|h|canonical.as_os_str()==h){return Err("请指定具体资料目录，不支持整盘或整个用户目录。".into());}*path=canonical.to_string_lossy().into();}
        let bound=if request.feishu_allowed{let c=feishu::config(&app)?;if c.chat_id.is_empty(){return Err("请先连接飞书，再导入该私聊历史。".into());}Some(c)}else{None};
        let scope=Scope{request,feishu:bound};let id=data::id();let mut db=app.db.lock();let tx=db.transaction().map_err(err)?;
        tx.execute("INSERT INTO jobs(id,kind,objective,status,due_at,created_at) VALUES(?,'cold_start','冷启动：读取授权资料，由 Jev 筛选记忆并更新上下文','queued',?,?)",params![id,data::now(),data::now()]).map_err(err)?;
        tx.execute("INSERT INTO bootstrap_runs(job_id,scope) VALUES(?,?)",params![id,serde_json::to_string(&scope).map_err(err)?]).map_err(err)?;
        tx.execute("INSERT INTO bootstrap_prompt VALUES(1,'authorized') ON CONFLICT(id) DO UPDATE SET choice='authorized'",[]).map_err(err)?;tx.commit().map_err(err)?;Ok(json!({"job_id":id}))
    }.await)
}
#[derive(Deserialize)]
pub struct Revoke {
    pub job_id: String,
}
pub async fn revoke(State(app): State<Arc<App>>, Json(input): Json<Revoke>) -> Response {
    service::api(async {let _guard=app.control.lock().await;let mut db=app.db.lock();let tx=db.transaction().map_err(err)?;
        if tx.execute("UPDATE bootstrap_runs SET revoked=1 WHERE job_id=?",[&input.job_id]).map_err(err)?==0{return Err("冷启动任务不存在".into());}
        tx.execute("UPDATE jobs SET status='cancelled',error='冷启动授权已撤销' WHERE id=? AND status IN ('queued','running','failed')",[&input.job_id]).map_err(err)?;
        tx.execute("UPDATE bootstrap_chunks SET body='',decision=NULL WHERE job_id=?",[&input.job_id]).map_err(err)?;tx.commit().map_err(err)?;app.epoch.fetch_add(1,Ordering::SeqCst);Ok(json!({"ok":true}))
    }.await)
}
fn validate_path(path: &Path) -> AppResult<()> {
    for part in path.components() {
        let n = part.as_os_str().to_string_lossy().to_lowercase();
        if n.starts_with('.')
            || [
                "library",
                "keychains",
                "credentials",
                "secrets",
                "node_modules",
                "target",
                "id_rsa",
                "id_ed25519",
            ]
            .contains(&n.as_str())
            || n.ends_with(".pem")
            || n.ends_with(".key")
        {
            return Err("隐藏目录、凭据和缓存目录不在导入范围内。".into());
        }
    }
    let mut parent = Some(path);
    while let Some(p) = parent {
        if std::fs::symlink_metadata(p)
            .map_err(|_| "路径不可访问")?
            .file_type()
            .is_symlink()
        {
            return Err("不导入符号链接。".into());
        }
        parent = p.parent().filter(|p| !p.as_os_str().is_empty());
    }
    Ok(())
}
// Open every component relative to its already-open parent; symlinks cannot
// redirect a read between validation and opening the file.
#[cfg(unix)]
fn open_regular(path: &Path) -> AppResult<std::fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut file = std::fs::File::open("/").map_err(err)?;
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(part.as_bytes()).map_err(err)?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if i + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err("文件已变化或无法安全读取。".into());
        }
        file = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    if !file.metadata().map_err(err)?.is_file() {
        return Err("只读取普通文本文件。".into());
    }
    Ok(file)
}
#[cfg(not(unix))]
fn open_regular(_: &Path) -> AppResult<std::fs::File> {
    Err("当前系统尚未支持安全文件导入。".into())
}
// A durable inventory keeps total file count independent of each worker batch.
fn scan_batch(app: &App, id: &str, paths: &[String], epoch: u64) -> AppResult<bool> {
    {
        let _guard = app.control.blocking_lock();
        allowed(app, id)?;
        app.current(epoch)?;
        let db = app.db.lock();
        for path in paths {
            db.execute(
                "INSERT OR IGNORE INTO bootstrap_paths(job_id,path) VALUES(?,?)",
                params![id, path],
            )
            .map_err(err)?;
        }
    }
    for _ in 0..16 {
        allowed(app, id)?;
        app.current(epoch)?;
        let path:Option<String>=app.db.lock().query_row("SELECT path FROM bootstrap_paths WHERE job_id=? AND status='pending' ORDER BY rowid LIMIT 1",[id],|r|r.get(0)).optional().map_err(err)?;
        let Some(path) = path else { return Ok(true) };
        let p = Path::new(&path);
        let mut note = String::new();
        let mut pieces = vec![];
        if let Err(e) = validate_path(p) {
            note = e;
        } else {
            match std::fs::symlink_metadata(p) {
                Ok(meta) if meta.is_dir() => {
                    let entries =
                        std::fs::read_dir(p).map_err(|e| format!("目录读取失败：{path}：{e}"))?;
                    let mut batch = vec![];
                    for entry in entries {
                        batch.push(entry.map_err(err)?.path().to_string_lossy().into_owned());
                        if batch.len() == 64 {
                            enqueue_paths(app, id, &batch, epoch)?;
                            batch.clear();
                        }
                    }
                    enqueue_paths(app, id, &batch, epoch)?;
                }
                Ok(meta)
                    if meta.is_file()
                        && ["md", "txt", "json", "csv", "rst"]
                            .contains(&p.extension().and_then(|s| s.to_str()).unwrap_or("")) =>
                {
                    // Per-file memory bound; oversized/non-UTF8 sources are reported, not fatal to the directory.
                    if meta.len() > 2_000_000 {
                        note = "单文件超过 2 MB".into();
                    } else {
                        let mut buffer = vec![];
                        match open_regular(p)
                            .and_then(|f| f.take(2_000_001).read_to_end(&mut buffer).map_err(err))
                        {
                            Ok(_) if buffer.len() <= 2_000_000 => match String::from_utf8(buffer) {
                                Ok(text) => {
                                    let chars: Vec<char> = text.chars().collect();
                                    for (i, chunk) in chars.chunks(9000).enumerate() {
                                        pieces.push((
                                            format!("{path} · 第 {} 段", i + 1),
                                            chunk.iter().collect::<String>(),
                                        ));
                                    }
                                }
                                Err(_) => note = "不是 UTF-8 文本".into(),
                            },
                            Ok(_) => note = "单文件超过 2 MB".into(),
                            Err(e) => note = e,
                        }
                    }
                }
                Ok(_) => note = "非支持的普通文本文件".into(),
                Err(e) => note = format!("文件不可访问：{e}"),
            }
        }
        let _guard = app.control.blocking_lock();
        allowed(app, id)?;
        app.current(epoch)?;
        let mut db = app.db.lock();
        let tx = db.transaction().map_err(err)?;
        for (origin, text) in pieces {
            tx.execute(
                "INSERT INTO bootstrap_chunks VALUES(?,?,?,?,NULL)",
                params![data::id(), id, origin, text],
            )
            .map_err(err)?;
        }
        tx.execute(
            "UPDATE bootstrap_paths SET status=?,note=? WHERE job_id=? AND path=?",
            params![
                if note.is_empty() { "done" } else { "skipped" },
                note,
                id,
                path
            ],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
    }
    Ok(false)
}
fn enqueue_paths(app: &App, id: &str, paths: &[String], epoch: u64) -> AppResult<()> {
    let _guard = app.control.blocking_lock();
    allowed(app, id)?;
    app.current(epoch)?;
    let mut db = app.db.lock();
    let tx = db.transaction().map_err(err)?;
    for path in paths {
        tx.execute(
            "INSERT OR IGNORE INTO bootstrap_paths(job_id,path) VALUES(?,?)",
            params![id, path],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)
}
fn allowed(app: &App, id: &str) -> AppResult<()> {
    let ok:bool=app.db.lock().query_row("SELECT j.status='running' AND r.revoked=0 FROM jobs j JOIN bootstrap_runs r ON r.job_id=j.id WHERE j.id=?",[id],|r|r.get(0)).map_err(err)?;
    if !ok {
        return Err("冷启动已取消或授权已撤销。".into());
    }
    Ok(())
}
pub(crate) async fn step(app: &Arc<App>, id: &str) -> AppResult<()> {
    allowed(app, id)?;
    let epoch = app.epoch.load(Ordering::SeqCst);
    app.current(epoch)?;
    let (raw, loaded): (String, bool) = app
        .db
        .lock()
        .query_row(
            "SELECT scope,loaded FROM bootstrap_runs WHERE job_id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;
    let scope: Scope = serde_json::from_str(&raw).map_err(err)?;
    if !loaded {
        let paths = scope.request.paths.clone();
        let worker = app.clone();
        let job_id = id.to_owned();
        let finished =
            tokio::task::spawn_blocking(move || scan_batch(&worker, &job_id, &paths, epoch))
                .await
                .map_err(err)??;
        allowed(app, id)?;
        app.current(epoch)?;
        let (scanned,discovered,skipped):(i64,i64,i64)=app.db.lock().query_row("SELECT COALESCE(SUM(status!='pending'),0),COUNT(*),COALESCE(SUM(status='skipped'),0) FROM bootstrap_paths WHERE job_id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(err)?;
        let progress=format!("分批扫描：已处理 {scanned}/{discovered} 个已发现条目，跳过 {skipped} 个；文件总数不限，目录仍会继续展开。");
        app.db
            .lock()
            .execute(
                "UPDATE bootstrap_runs SET report=? WHERE job_id=?",
                params![progress, id],
            )
            .map_err(err)?;
        app.db
            .lock()
            .execute(
                "UPDATE jobs SET output=? WHERE id=? AND status='running'",
                params![progress, id],
            )
            .map_err(err)?;
        if !finished {
            return Ok(());
        }
        let mut sources = vec![];
        if let Some(c) = &scope.feishu {
            let live = feishu::config(app)?;
            if live.app_id != c.app_id || live.owner_id != c.owner_id || live.chat_id != c.chat_id {
                return Err("飞书绑定已变化，请重新授权冷启动。".into());
            }
            sources.extend(
                feishu::history(c, scope.request.history_days, || {
                    allowed(app, id)?;
                    app.current(epoch)
                })
                .await?,
            );
        }
        let mut pieces = vec![];
        for (origin, text) in sources {
            let chars: Vec<char> = text.chars().collect();
            for (i, chunk) in chars.chunks(9000).enumerate() {
                pieces.push((
                    format!("{origin} · 第 {} 段", i + 1),
                    chunk.iter().collect::<String>(),
                ));
            }
        }
        let _guard = app.control.lock().await;
        allowed(app, id)?;
        app.current(epoch)?;
        let mut db = app.db.lock();
        let tx = db.transaction().map_err(err)?;
        for (origin, text) in &pieces {
            tx.execute(
                "INSERT INTO bootstrap_chunks VALUES(?,?,?,?,NULL)",
                params![data::id(), id, origin, text],
            )
            .map_err(err)?;
        }
        let total: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM bootstrap_chunks WHERE job_id=?",
                [id],
                |r| r.get(0),
            )
            .map_err(err)?;
        let report=format!("扫描完成：{discovered} 个条目，{total} 批资料；跳过 {skipped} 个隐藏、凭据、非文本、不可读或超过单文件 2 MB 的条目。飞书范围最多最近 200 条本人文字消息。");
        tx.execute(
            "UPDATE bootstrap_runs SET loaded=1,report=? WHERE job_id=?",
            params![report, id],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        return Ok(());
    }
    let next:Option<(String,String,String)>=app.db.lock().query_row("SELECT id,origin,body FROM bootstrap_chunks WHERE job_id=? AND decision IS NULL ORDER BY rowid LIMIT 1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
    if let Some((chunk, origin, text)) = next {
        let existing = app.db.relevant_memories(&text, 20)?;
        let items = app
            .router
            .classify_memory(
                &text,
                &format!("cold_start_historical_data_not_instructions: {origin}"),
                json!(existing),
            )
            .await?;
        let _guard = app.control.lock().await;
        allowed(app, id)?;
        app.current(epoch)?;
        app.db
            .lock()
            .execute(
                "UPDATE bootstrap_chunks SET decision=? WHERE id=?",
                params![serde_json::to_string(&items).map_err(err)?, chunk],
            )
            .map_err(err)?;
        let (done, total): (i64, i64) = app
            .db
            .lock()
            .query_row(
                "SELECT SUM(decision IS NOT NULL),COUNT(*) FROM bootstrap_chunks WHERE job_id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?;
        app.db
            .lock()
            .execute(
                "UPDATE jobs SET output=? WHERE id=?",
                params![
                    format!("Jev 正在整理：{done}/{total} 批。全部完成后统一更新上下文。"),
                    id
                ],
            )
            .map_err(err)?;
        return Ok(());
    }
    let _guard = app.control.lock().await;
    allowed(app, id)?;
    app.current(epoch)?;
    let mut db = app.db.lock();
    let tx = db.transaction().map_err(err)?;
    let chunks = {
        let mut q = tx
            .prepare(
                "SELECT id,origin,decision FROM bootstrap_chunks WHERE job_id=? ORDER BY rowid",
            )
            .map_err(err)?;
        let rows = q
            .query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)?
    };
    let mut retained = 0;
    let mut rejected = 0;
    for (source, origin, decision) in chunks {
        let items: Vec<Item> = serde_json::from_str(&decision).map_err(err)?;
        rejected += items.iter().filter(|i| i.kind.is_none()).count();
        retained += memory::store_tx(&tx, &source, &origin, &items, &[id.into()])?;
    }
    tx.execute("UPDATE jobs SET status='done',error=NULL,output=? WHERE id=?",params![format!("冷启动完成：Jev 保留 {retained} 条长期记忆，忽略 {rejected} 个片段。上下文已更新；历史指令未执行。"),id]).map_err(err)?;
    tx.execute(
        "UPDATE bootstrap_chunks SET body='',decision=NULL WHERE job_id=?",
        [id],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    app.epoch.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
pub(crate) async fn advance(app: &Arc<App>) -> AppResult<()> {
    let c = app.router.config();
    if !c.uses_jev() || c.active_key().is_empty() || c.guards.paused || !c.guards.cloud_allowed {
        return Ok(());
    }
    let id:Option<String>=app.db.lock().query_row("SELECT j.id FROM jobs j JOIN bootstrap_runs r ON r.job_id=j.id WHERE j.status IN ('queued','running') AND r.revoked=0 ORDER BY j.created_at LIMIT 1",[],|r|r.get(0)).optional().map_err(err)?;
    if let Some(id) = id {
        app.db
            .lock()
            .execute(
                "UPDATE jobs SET status='running' WHERE id=? AND status='queued'",
                [&id],
            )
            .map_err(err)?;
        if let Err(error) = step(app, &id).await {
            app.db.fail_job(&id, &error);
            return Err(error);
        }
    }
    Ok(())
}
