use mind_runtime::*;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

struct ReplayClock(AtomicI64);
impl Clock for ReplayClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn read_events(path: &str) -> std::result::Result<Vec<Event>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut events = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let event: Event =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        event.validate()?;
        if events
            .last()
            .is_some_and(|previous: &Event| previous.created_at_ms > event.created_at_ms)
        {
            return Err(format!("line {}: replay timestamps must be nondecreasing", i + 1).into());
        }
        events.push(event);
    }
    if events.is_empty() {
        return Err("empty replay".into());
    }
    Ok(events)
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!("Shadow-only, offline mock runtime. No messages are sent.\n\n  mind-runtime replay EVENTS.jsonl --db NEW.sqlite [--guards GUARDS.json]\n  mind-runtime inspect DATABASE.sqlite\n\nReplay uses each event's created_at_ms as its clock. Guards come only from the explicit host config.");
        return Ok(());
    }
    if args[0] == "inspect" && args.len() == 2 {
        let db = rusqlite::Connection::open_with_flags(
            &args[1],
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut statement = db.prepare("SELECT payload FROM decisions ORDER BY rowid")?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            println!("{}", row?);
        }
        return Ok(());
    }
    if args[0] != "replay"
        || !(args.len() == 4 || args.len() == 6)
        || args[2] != "--db"
        || (args.len() == 6 && args[4] != "--guards")
    {
        return Err("invalid arguments; use --help".into());
    }
    let events = read_events(&args[1])?;
    let guards: Guards = if args.len() == 6 {
        serde_json::from_str(&std::fs::read_to_string(&args[5])?)?
    } else {
        Guards::default()
    };
    guards.validate()?;
    let path = Path::new(&args[3]);
    // Replay always starts with a clean state; never silently overwrites a live database.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    drop(file);
    let clock = Arc::new(ReplayClock(AtomicI64::new(events[0].created_at_ms)));
    let runtime = Runtime::spawn(
        Store::open(path, Persona::default(), clock.now_ms())?,
        Arc::new(MockProvider),
        clock.clone(),
    )?;
    runtime.set_guards(guards).await?;
    for event in events {
        clock.0.store(event.created_at_ms, Ordering::SeqCst);
        println!("{}", serde_json::to_string(&runtime.submit(event).await?)?);
    }
    runtime.shutdown().await?;
    Ok(())
}
