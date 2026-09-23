use crate::{model::*, policy, Error, Result, Store};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

type Reply<T> = oneshot::Sender<Result<T>>;
enum Command {
    Submit(Event, Reply<Decision>),
    Guards(Guards, Reply<()>),
    Decisions(Reply<Vec<Decision>>),
    Shutdown(Reply<()>),
    Status(Reply<MindStatus>),
    Forget(Vec<String>, Reply<()>),
}
#[derive(Clone)]
pub struct Runtime {
    commands: mpsc::Sender<Command>,
}
impl Runtime {
    pub fn spawn(
        store: Store,
        provider: Arc<dyn DecisionProvider>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        Self::spawn_with_timeout(store, provider, clock, Duration::from_secs(5))
    }
    pub fn spawn_with_timeout(
        store: Store,
        provider: Arc<dyn DecisionProvider>,
        clock: Arc<dyn Clock>,
        timeout: Duration,
    ) -> Result<Self> {
        let queue = store.pending()?.into();
        let (commands, receiver) = mpsc::channel(64);
        tokio::spawn(async move {
            Actor {
                store,
                provider,
                clock,
                queue,
                replies: HashMap::new(),
                timeout,
            }
            .run(receiver)
            .await;
        });
        Ok(Self { commands })
    }
    pub async fn submit(&self, event: Event) -> Result<Decision> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Submit(event, tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
    /// Trusted host control plane only; never expose this as a model tool.
    pub async fn set_guards(&self, guards: Guards) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Guards(guards, tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
    pub async fn decisions(&self) -> Result<Vec<Decision>> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Decisions(tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
    pub async fn status(&self) -> Result<MindStatus> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Status(tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
    pub async fn forget_events(&self, ids: Vec<String>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Forget(ids, tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Shutdown(tx))
            .await
            .map_err(|_| Error::Stopped)?;
        rx.await.map_err(|_| Error::Stopped)?
    }
}
struct Actor {
    store: Store,
    provider: Arc<dyn DecisionProvider>,
    clock: Arc<dyn Clock>,
    queue: VecDeque<Event>,
    replies: HashMap<String, Reply<Decision>>,
    timeout: Duration,
}
impl Actor {
    fn gate(&self, event: &Event) -> Result<Option<&'static str>> {
        let now = self.clock.now_ms();
        let (count, last) = if let Some(c) = &event.candidate {
            self.store.budget(c.kind, now)?
        } else {
            (0, None)
        };
        if let Some(reason) = policy::hard_gate(event, &self.store.guards()?, now, count, last) {
            return Ok(Some(reason));
        }
        if event.kind == EventKind::UserMessage {
            return Ok(None);
        }
        if let Some(c) = &event.candidate {
            if self.store.candidate_used(&c.id)? {
                return Ok(Some("candidate_already_evaluated"));
            }
        }
        if self.provider.remote() && !self.store.guards()?.cloud_allowed {
            return Ok(Some("cloud_not_authorized"));
        }
        Ok(None)
    }
    fn complete(&mut self, e: &Event, d: Decision, a: Option<&Appraisal>, affect: Option<Affect>) {
        let result = self
            .store
            .finish(e, &d, a, affect, self.clock.now_ms())
            .map(|_| d);
        if let Some(reply) = self.replies.remove(&e.id) {
            let _ = reply.send(result);
        } else if let Err(error) = result {
            eprintln!("recovery commit failed: {error}");
        }
    }
    fn handle(&mut self, cmd: Command) -> Option<Reply<()>> {
        match cmd {
            Command::Submit(e, reply) => {
                if let Err(error) = e.validate() {
                    let _ = reply.send(Err(error));
                    return None;
                }
                match self.store.guards() {
                    Ok(g) if !g.allowed_sources.contains(&e.source) => {
                        let _ = reply.send(Ok(Decision::new(
                            &e,
                            Action::Wait,
                            "source_not_authorized",
                        )));
                        return None;
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        return None;
                    }
                    _ => {}
                }
                match self.store.ingest(&e) {
                    Ok(true) => {
                        self.replies.insert(e.id.clone(), reply);
                        self.queue.push_back(e);
                    }
                    Ok(false) => {
                        let _ = reply.send(Ok(Decision::new(
                            &e,
                            Action::Duplicate,
                            "event_already_seen",
                        )));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::Guards(g, reply) => {
                let _ = reply.send(self.store.set_guards(&g));
            }
            Command::Decisions(reply) => {
                let _ = reply.send(self.store.decisions());
            }
            Command::Status(reply) => {
                let _ = reply.send((|| {
                    Ok(MindStatus {
                        revision: self.store.revision()?,
                        affect: self.store.affect()?,
                        guards: self.store.guards()?,
                    })
                })());
            }
            Command::Forget(ids, reply) => {
                let result = self.store.forget_events(&ids, self.clock.now_ms());
                if let Ok(removed) = &result {
                    self.queue.retain(|event| !removed.contains(&event.id));
                    for id in removed {
                        if let Some(waiter) = self.replies.remove(id) {
                            let _ = waiter.send(Err(Error::Invalid("source deleted".into())));
                        }
                    }
                }
                let _ = reply.send(result.map(|_| ()));
            }
            Command::Shutdown(reply) => return Some(reply),
        }
        None
    }
    async fn run(mut self, mut receiver: mpsc::Receiver<Command>) {
        let mut shutdown = None;
        'actor: loop {
            let Some(event) = self.queue.pop_front() else {
                match receiver.recv().await {
                    Some(cmd) => {
                        if let Some(reply) = self.handle(cmd) {
                            shutdown = Some(reply);
                            break;
                        }
                    }
                    None => break,
                }
                continue;
            };
            let start = self.gate(&event).and_then(|reason| {
                if let Some(reason) = reason {
                    return Ok(Err(reason));
                }
                if event.kind == EventKind::UserMessage {
                    return Ok(Err("user_initiated"));
                }
                self.store.mark_processing(&event.id)?;
                self.store.snapshot(&event, self.clock.now_ms()).map(Ok)
            });
            let snapshot = match start {
                Ok(Ok(s)) => s,
                Ok(Err(reason)) => {
                    self.complete(
                        &event,
                        Decision::new(
                            &event,
                            if reason == "user_initiated" {
                                Action::Respond
                            } else {
                                Action::Wait
                            },
                            reason,
                        ),
                        None,
                        None,
                    );
                    continue;
                }
                Err(error) => {
                    self.complete(
                        &event,
                        Decision::new(&event, Action::Wait, &error.to_string()),
                        None,
                        None,
                    );
                    continue;
                }
            };
            let provider = self.provider.clone();
            let evaluation =
                tokio::time::timeout(self.timeout, provider.evaluate(snapshot.clone()));
            tokio::pin!(evaluation);
            // Only this actor writes SQLite. Host commands remain responsive during I/O.
            let output = loop {
                tokio::select! {
                    cmd = receiver.recv() => match cmd {
                        Some(cmd) => if let Some(reply) = self.handle(cmd) { shutdown = Some(reply); break 'actor; },
                        None => break 'actor,
                    },
                    result = &mut evaluation => break result,
                }
            };
            let outcome = (|| -> Result<(Decision, Option<Appraisal>, Option<Affect>)> {
                if snapshot.revision != self.store.revision()? {
                    return Ok((
                        Decision::new(&event, Action::Stale, "state_changed_during_decision"),
                        None,
                        None,
                    ));
                }
                if let Some(reason) = self.gate(&event)? {
                    return Ok((Decision::new(&event, Action::Wait, reason), None, None));
                }
                self.store.snapshot(&event, self.clock.now_ms())?; // recheck all evidence TTLs
                let a = match output {
                    Ok(Ok(a)) => a,
                    _ => {
                        return Ok((
                            Decision::new(&event, Action::Wait, "decision_unavailable"),
                            None,
                            None,
                        ))
                    }
                };
                a.validate()?;
                let affect = policy::reduce(snapshot.affect, &a, self.clock.now_ms());
                Ok((policy::decide(&event, &a, affect), Some(a), Some(affect)))
            })();
            match outcome {
                Ok((d, a, affect)) => self.complete(&event, d, a.as_ref(), affect),
                Err(error) => self.complete(
                    &event,
                    Decision::new(&event, Action::Wait, &error.to_string()),
                    None,
                    None,
                ),
            }
        }
        // Drop releases the DB lock before acknowledging shutdown. Pending work survives.
        drop(self);
        if let Some(reply) = shutdown {
            let _ = reply.send(Ok(()));
        }
    }
}
