use mind_runtime::*;
use std::sync::{
    atomic::{AtomicI64, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Semaphore;

const NOW: i64 = 1_790_035_200_000;
struct TestClock(AtomicI64);
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Provider {
    calls: AtomicUsize,
    remote: bool,
    entered: Semaphore,
    release: Option<Semaphore>,
    value: f64,
    evidence: f64,
}
impl Provider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            remote: false,
            entered: Semaphore::new(0),
            release: None,
            value: 0.96,
            evidence: 0.96,
        }
    }
}
impl DecisionProvider for Provider {
    fn remote(&self) -> bool {
        self.remote
    }
    fn evaluate(&self, snapshot: Snapshot) -> ProviderFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            if let Some(release) = &self.release {
                release.acquire().await.unwrap().forget();
            }
            let mut appraisal = MockProvider.evaluate(snapshot).await?;
            appraisal.benefit = self.value;
            appraisal.evidence_sufficient = self.evidence;
            Ok(appraisal)
        })
    }
}
fn event(id: &str) -> Event {
    Event {
        id: id.into(),
        source: "work".into(),
        kind: EventKind::WorkResult,
        text: "Render completed".into(),
        created_at_ms: NOW,
        expires_at_ms: NOW + 300_000,
        candidate: Some(Candidate {
            id: id.into(),
            kind: InitiativeKind::TaskUpdate,
            proposition: "Video is ready".into(),
            evidence_ids: vec![id.into()],
        }),
    }
}
fn guards() -> Guards {
    let mut g = Guards::default();
    g.kinds[0].consent = true;
    g.kinds[0].cooldown_ms = 0;
    g
}
async fn setup(provider: Arc<Provider>) -> (TempDir, Runtime, Arc<TestClock>) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock(AtomicI64::new(NOW)));
    let runtime = Runtime::spawn(
        Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap(),
        provider,
        clock.clone(),
    )
    .unwrap();
    runtime.set_guards(guards()).await.unwrap();
    (dir, runtime, clock)
}

#[tokio::test]
async fn shadow_deduplicates_events_and_candidates() {
    let p = Arc::new(Provider::new());
    let (_dir, rt, _) = setup(p.clone()).await;
    let first = rt.submit(event("one")).await.unwrap();
    assert_eq!(first.action, Action::Speak);
    assert!(first.shadow);
    assert_eq!(
        rt.submit(event("one")).await.unwrap().action,
        Action::Duplicate
    );
    let mut second = event("two");
    second.candidate.as_mut().unwrap().id = "one".into();
    assert_eq!(
        rt.submit(second).await.unwrap().reason,
        "candidate_already_evaluated"
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn consent_is_per_kind_and_direct_bypasses_proactive_gates() {
    let p = Arc::new(Provider::new());
    let (_dir, rt, _) = setup(p.clone()).await;
    let mut e = event("social");
    e.candidate.as_mut().unwrap().kind = InitiativeKind::SocialCheckIn;
    assert_eq!(rt.submit(e).await.unwrap().reason, "kind_not_authorized");
    let mut g = Guards {
        quiet: true,
        busy: true,
        ..Guards::default()
    };
    rt.set_guards(g.clone()).await.unwrap();
    let mut e = event("user");
    e.source = "user".into();
    e.kind = EventKind::UserMessage;
    e.candidate = None;
    assert_eq!(rt.submit(e.clone()).await.unwrap().action, Action::Respond);
    g.paused = true;
    rt.set_guards(g).await.unwrap();
    e.id = "paused-user".into();
    assert_eq!(rt.submit(e).await.unwrap().reason, "paused");
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn unauthorized_source_never_persisted() {
    let (dir, rt, _) = setup(Arc::new(Provider::new())).await;
    let mut e = event("secret");
    e.source = "private-mail".into();
    assert_eq!(rt.submit(e).await.unwrap().reason, "source_not_authorized");
    assert!(rt.decisions().await.unwrap().is_empty());
    rt.shutdown().await.unwrap();
    let db = rusqlite::Connection::open(dir.path().join("mind.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM inbox", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn revocation_remains_responsive_and_invalidates_inflight_result() {
    let mut provider = Provider::new();
    provider.release = Some(Semaphore::new(0));
    let p = Arc::new(provider);
    let (_dir, rt, _) = setup(p.clone()).await;
    let submit = tokio::spawn({
        let rt = rt.clone();
        async move { rt.submit(event("one")).await.unwrap() }
    });
    p.entered.acquire().await.unwrap().forget();
    tokio::time::timeout(Duration::from_secs(1), rt.set_guards(Guards::default()))
        .await
        .unwrap()
        .unwrap();
    p.release.as_ref().unwrap().add_permits(1);
    assert_eq!(submit.await.unwrap().action, Action::Stale);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn evidence_expiring_during_provider_call_prevents_commit() {
    let mut provider = Provider::new();
    provider.release = Some(Semaphore::new(0));
    let p = Arc::new(provider);
    let (_dir, rt, clock) = setup(p.clone()).await;
    let mut evidence = event("evidence");
    evidence.candidate = None;
    evidence.expires_at_ms = NOW + 1;
    rt.submit(evidence).await.unwrap();
    let mut e = event("one");
    e.candidate.as_mut().unwrap().evidence_ids = vec!["evidence".into()];
    let submit = tokio::spawn({
        let rt = rt.clone();
        async move { rt.submit(e).await.unwrap() }
    });
    p.entered.acquire().await.unwrap().forget();
    clock.0.store(NOW + 2, Ordering::SeqCst);
    p.release.as_ref().unwrap().add_permits(1);
    let result = submit.await.unwrap();
    assert_eq!(result.action, Action::Wait);
    assert!(result.reason.contains("evidence_expired"));
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn cloud_denied_and_invalid_probability_fail_closed() {
    let mut p = Provider::new();
    p.remote = true;
    let p = Arc::new(p);
    let (_dir, rt, _) = setup(p.clone()).await;
    assert_eq!(
        rt.submit(event("one")).await.unwrap().reason,
        "cloud_not_authorized"
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    rt.shutdown().await.unwrap();
    let mut p = Provider::new();
    p.value = f64::NAN;
    let (_dir, rt, _) = setup(Arc::new(p)).await;
    assert_eq!(rt.submit(event("one")).await.unwrap().action, Action::Wait);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn low_evidence_records_reflection_only() {
    let mut p = Provider::new();
    p.evidence = 0.4;
    let (_dir, rt, _) = setup(Arc::new(p)).await;
    assert_eq!(
        rt.submit(event("one")).await.unwrap().action,
        Action::Reflect
    );
    assert!(rt.decisions().await.unwrap()[0].shadow);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn rolling_budget_survives_restart_and_expires() {
    let (dir, rt, clock) = setup(Arc::new(Provider::new())).await;
    let mut g = guards();
    g.kinds[0].daily_limit = 1;
    rt.set_guards(g).await.unwrap();
    rt.submit(event("one")).await.unwrap();
    rt.shutdown().await.unwrap();
    let rt = Runtime::spawn(
        Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap(),
        Arc::new(MockProvider),
        clock.clone(),
    )
    .unwrap();
    assert_eq!(
        rt.submit(event("two")).await.unwrap().reason,
        "daily_budget"
    );
    clock.0.store(NOW + 86_400_001, Ordering::SeqCst);
    let mut e = event("three");
    e.created_at_ms = clock.now_ms();
    e.expires_at_ms = e.created_at_ms + 300_000;
    assert_eq!(rt.submit(e).await.unwrap().action, Action::Speak);
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupted_processing_is_recovered_without_duplicate_affect() {
    let mut p = Provider::new();
    p.release = Some(Semaphore::new(0));
    let p = Arc::new(p);
    let (dir, rt, clock) = setup(p.clone()).await;
    let submit = tokio::spawn({
        let rt = rt.clone();
        async move { rt.submit(event("one")).await }
    });
    p.entered.acquire().await.unwrap().forget();
    rt.shutdown().await.unwrap();
    assert!(submit.await.unwrap().is_err());
    let recovered = Runtime::spawn(
        Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap(),
        Arc::new(MockProvider),
        clock,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if recovered.decisions().await.unwrap().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    recovered.shutdown().await.unwrap();
    let store = Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap();
    assert!(store.pending().unwrap().is_empty());
    let affect = store.affect().unwrap();
    drop(store);
    let store = Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap();
    assert_eq!(store.affect().unwrap(), affect);
    assert_eq!(store.decisions().unwrap().len(), 1);
}

#[tokio::test]
async fn provider_timeout_is_durable_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("mind.sqlite"), Persona::default(), NOW).unwrap();
    store.set_guards(&guards()).unwrap();
    let mut p = Provider::new();
    p.release = Some(Semaphore::new(0));
    let rt = Runtime::spawn_with_timeout(
        store,
        Arc::new(p),
        Arc::new(TestClock(AtomicI64::new(NOW))),
        Duration::from_millis(10),
    )
    .unwrap();
    assert_eq!(
        rt.submit(event("one")).await.unwrap().reason,
        "decision_unavailable"
    );
    rt.shutdown().await.unwrap();
}

#[test]
fn store_excludes_other_writers_and_persona_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mind.sqlite");
    let store = Store::open(&path, Persona::default(), NOW).unwrap();
    assert!(Store::open(&path, Persona::default(), NOW).is_err());
    drop(store);
    let persona = Persona {
        version: "2".into(),
        ..Persona::default()
    };
    assert!(Store::open(&path, persona, NOW).is_err());
}

#[test]
fn affect_changes_decisions_but_never_bypasses_interruption_gate() {
    let e = event("one");
    let mut a = Appraisal {
        selected_action: None,
        benefit: 0.84,
        novelty: 0.7,
        interruption: 0.08,
        interest: 0.61,
        goal_congruence: 0.8,
        surprise: 0.5,
        evidence_sufficient: 0.95,
        model: "fixture".into(),
        rubric_version: RUBRIC_VERSION.into(),
    };
    let cold = Affect {
        valence: -1.,
        arousal: 0.,
        control: 0.5,
        updated_at_ms: NOW,
    };
    let warm = Affect {
        valence: 1.,
        arousal: 1.,
        ..cold
    };
    assert_ne!(
        policy::decide(&e, &a, cold).action,
        policy::decide(&e, &a, warm).action
    );
    a.interruption = 0.9;
    assert_eq!(policy::decide(&e, &a, warm).action, Action::Wait);
    let mut affect = warm;
    for i in 0..1000 {
        affect = policy::reduce(affect, &a, NOW + i * 1000);
    }
    assert!((-1.0..=1.0).contains(&affect.valence));
    assert!((0.0..=1.0).contains(&affect.arousal));
}

// Child process exits without running destructors to exercise actual lock/SQLite recovery.
#[test]
fn crash_writer_child() {
    let Some(path) = std::env::var_os("YOURSELF_CRASH_TEST_DB") else {
        return;
    };
    let mut store = Store::open(path, Persona::default(), NOW).unwrap();
    store.set_guards(&guards()).unwrap();
    store.ingest(&event("crashed")).unwrap();
    store.mark_processing("crashed").unwrap();
    std::process::exit(86);
}

#[tokio::test]
async fn abrupt_process_exit_releases_lock_and_replays_durable_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mind.sqlite");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_writer_child", "--nocapture"])
        .env("YOURSELF_CRASH_TEST_DB", &path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(86));
    let store = Store::open(&path, Persona::default(), NOW).unwrap();
    assert_eq!(store.pending().unwrap().len(), 1);
    let rt = Runtime::spawn(
        store,
        Arc::new(MockProvider),
        Arc::new(TestClock(AtomicI64::new(NOW))),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let decisions = rt.decisions().await.unwrap();
            if !decisions.is_empty() {
                assert_eq!(decisions[0].action, Action::Speak);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    rt.shutdown().await.unwrap();
}

#[test]
fn ai_cadence_ignores_time_and_count_preferences() {
    let mut g = guards();
    g.ai_driven = true;
    g.paused = true;
    g.quiet = true;
    g.busy = true;
    for k in &mut g.kinds {
        k.consent = false;
        k.daily_limit = 0;
        k.cooldown_ms = i64::MAX;
    }
    assert_eq!(
        mind_runtime::policy::hard_gate(&event("autonomous"), &g, NOW, u32::MAX, Some(NOW)),
        None
    );
}
