use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

pub const POLICY_VERSION: &str = "shadow-1";
pub const RUBRIC_VERSION: &str = "atomic-appraisal-1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Persona {
    pub id: String,
    pub version: String,
    pub name: String,
    pub values: Vec<String>,
    pub interests: Vec<String>,
}
impl Default for Persona {
    fn default() -> Self {
        Self {
            id: "original-digital-researcher".into(),
            version: "1".into(),
            name: "知微".into(),
            values: vec!["求证".into(), "尊重注意力".into()],
            interests: vec!["智能体".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InitiativeKind {
    TaskUpdate,
    PersonalDiscovery,
    CuriosityQuestion,
    SocialCheckIn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindPolicy {
    pub kind: InitiativeKind,
    pub consent: bool,
    pub daily_limit: u32,
    pub cooldown_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    #[serde(default)]
    pub ai_driven: bool,
    pub paused: bool,
    pub quiet: bool,
    pub busy: bool,
    pub cloud_allowed: bool,
    pub allowed_sources: Vec<String>,
    pub kinds: Vec<KindPolicy>,
}
impl Default for Guards {
    fn default() -> Self {
        Self {
            ai_driven: false,
            paused: false,
            quiet: false,
            busy: false,
            cloud_allowed: false,
            allowed_sources: vec!["user".into(), "work".into(), "browser".into()],
            kinds: [
                InitiativeKind::TaskUpdate,
                InitiativeKind::PersonalDiscovery,
                InitiativeKind::CuriosityQuestion,
                InitiativeKind::SocialCheckIn,
            ]
            .into_iter()
            .map(|kind| KindPolicy {
                kind,
                consent: false,
                daily_limit: 3,
                cooldown_ms: 1_800_000,
            })
            .collect(),
        }
    }
}
impl Guards {
    pub fn validate(&self) -> Result<()> {
        for (i, p) in self.kinds.iter().enumerate() {
            if p.cooldown_ms < 0 || self.kinds[..i].iter().any(|q| q.kind == p.kind) {
                return Err(Error::Invalid(
                    "negative cooldown or duplicate initiative policy".into(),
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: String,
    pub kind: InitiativeKind,
    pub proposition: String,
    pub evidence_ids: Vec<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UserMessage,
    Observation,
    WorkResult,
    Wake,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    pub source: String,
    pub kind: EventKind,
    pub text: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub candidate: Option<Candidate>,
}
impl Event {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 128
            || self.source.is_empty()
            || self.text.len() > 16000
            || self.created_at_ms < 0
            || self.expires_at_ms <= self.created_at_ms
            || (self.kind == EventKind::UserMessage && self.source != "user")
        {
            return Err(Error::Invalid(
                "event identity, source, text or time".into(),
            ));
        }
        if let Some(c) = &self.candidate {
            if c.id.is_empty()
                || c.id.len() > 128
                || c.proposition.trim().is_empty()
                || c.proposition.len() > 4000
                || c.evidence_ids.is_empty()
                || c.evidence_ids.len() > 32
            {
                return Err(Error::Invalid(
                    "candidate requires bounded text and evidence".into(),
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Affect {
    pub valence: f64,
    pub arousal: f64,
    pub control: f64,
    pub updated_at_ms: i64,
}
impl Affect {
    pub fn initial(now: i64) -> Self {
        Self {
            valence: 0.,
            arousal: 0.2,
            control: 0.5,
            updated_at_ms: now,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub persona: Persona,
    pub affect: Affect,
    pub event: Event,
    pub evidence: Vec<Event>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appraisal {
    #[serde(default)]
    pub selected_action: Option<Action>,
    pub benefit: f64,
    pub novelty: f64,
    pub interruption: f64,
    pub interest: f64,
    pub goal_congruence: f64,
    pub surprise: f64,
    pub evidence_sufficient: f64,
    pub model: String,
    pub rubric_version: String,
}
impl Appraisal {
    pub fn validate(&self) -> Result<()> {
        if [
            self.benefit,
            self.novelty,
            self.interruption,
            self.interest,
            self.goal_congruence,
            self.surprise,
            self.evidence_sufficient,
        ]
        .iter()
        .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
            || self
                .selected_action
                .is_some_and(|a| !matches!(a, Action::Speak | Action::Wait | Action::Reflect))
            || self.model.is_empty()
            || self.rubric_version != RUBRIC_VERSION
        {
            return Err(Error::Invalid("invalid appraisal or rubric version".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Wait,
    Reflect,
    Speak,
    Respond,
    Stale,
    Duplicate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub event_id: String,
    pub action: Action,
    pub reason: String,
    pub shadow: bool,
    pub policy_version: String,
    pub utility: f64,
}
impl Decision {
    pub fn new(event: &Event, action: Action, reason: &str) -> Self {
        Self {
            event_id: event.id.clone(),
            action,
            reason: reason.into(),
            shadow: true,
            policy_version: POLICY_VERSION.into(),
            utility: 0.,
        }
    }
}
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<Appraisal, String>> + Send + 'a>>;
pub trait DecisionProvider: Send + Sync + 'static {
    fn remote(&self) -> bool;
    fn evaluate(&self, snapshot: Snapshot) -> ProviderFuture<'_>;
}
/// Explicit fixture, not a semantic model. Never reaches a network or delivery sink.
pub struct MockProvider;
impl DecisionProvider for MockProvider {
    fn remote(&self) -> bool {
        false
    }
    fn evaluate(&self, _: Snapshot) -> ProviderFuture<'_> {
        Box::pin(async {
            Ok(Appraisal {
                selected_action: None,
                benefit: 0.96,
                novelty: 0.95,
                interruption: 0.08,
                interest: 0.9,
                goal_congruence: 0.85,
                surprise: 0.5,
                evidence_sufficient: 0.96,
                model: "offline-mock-1".into(),
                rubric_version: RUBRIC_VERSION.into(),
            })
        })
    }
}
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MindStatus {
    pub revision: u64,
    pub affect: Affect,
    pub guards: Guards,
}
