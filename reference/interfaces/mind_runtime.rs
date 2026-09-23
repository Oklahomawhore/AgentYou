//! Production boundary proposal for the user's existing Rust/Tokio runtime.
//! Interface sketch only: not compiled in the preparation environment.
//! No dependency on a particular model SDK, no independent Trigger facade.
use std::{future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision(pub u64);
#[derive(Clone, Debug)]
pub struct EvidenceRef {
    pub event_id: String,
    pub source_id: String,
    pub valid_from_utc_ms: i64,
    pub valid_until_utc_ms: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct Belief {
    pub proposition: String,
    pub confidence: f64,
    pub evidence: Vec<EvidenceRef>,
    pub attributed_to: Option<String>, // Actor belief is not necessarily world truth.
}
#[derive(Clone, Debug)]
pub struct Affect {
    pub valence: f64,
    pub arousal: f64,
    pub control: f64,
    pub updated_at_utc_ms: i64,
}
#[derive(Clone, Debug)]
pub struct Drives {
    pub curiosity: f64,
    pub competence: f64,
    pub affiliation: f64,
    pub unfinished_commitments: f64,
}
#[derive(Clone, Debug)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    pub origin: GoalOrigin,
    pub authorization_scope: String,
    pub remaining_model_budget: u64,
}
#[derive(Clone, Debug)]
pub enum GoalOrigin { User, PreauthorizedSelfDirected }
#[derive(Clone, Debug)]
pub struct MindState {
    pub persona_id: String,
    pub persona_version: String,
    pub revision: Revision,
    pub affect: Affect,
    pub drives: Drives,
    pub beliefs: Vec<Belief>,
    pub goals: Vec<Goal>,
    // Memory bodies live in a store; use evidence IDs rather than duplicating history.
}
#[derive(Clone, Debug)]
pub enum InitiativeKind { TaskUpdate, PersonalDiscovery, CuriosityQuestion, SocialCheckIn }
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub kind: InitiativeKind,
    pub proposition: String,
    pub evidence: Vec<EvidenceRef>,
    pub expires_at_utc_ms: i64,
}
#[derive(Clone, Debug)]
pub struct ContextSnapshot {
    pub revision: Revision,
    pub perception_epoch: u64,
    pub dialogue_epoch: u64,
    pub permission_epoch: u64,
    pub state_json: Arc<str>, // Immutable projection, not the entire memory store.
    pub candidate: Option<Candidate>,
}
#[derive(Clone, Debug)]
pub struct Appraisal {
    pub welcome_probability: f64,
    pub relevance: f64,
    pub novelty: f64,
    pub interruption_risk: f64,
    pub evidence_sufficient: f64,
    pub goal_congruence: f64,
    pub model_version: String,
    pub rubric_version: String,
}
#[derive(Clone, Debug)]
pub enum Intent {
    Wait { reason: String },
    Defer { candidate_id: String, next_condition: String },
    Reflect { objective: String, evidence: Vec<EvidenceRef> },
    Speak { candidate_id: String },
    ProposeWork { goal_id: String },
}
#[derive(Clone, Debug)]
pub struct Draft {
    pub text: String,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug)]
pub enum ModelError { Timeout, Unavailable, InvalidOutput, VersionMismatch }

pub trait SystemOne: Send + Sync {
    fn evaluate<'a>(&'a self, snapshot: &'a ContextSnapshot)
        -> BoxFuture<'a, Result<Appraisal, ModelError>>;
}
pub trait MainDialogue: Send + Sync {
    fn draft<'a>(&'a self, snapshot: &'a ContextSnapshot, intent: &'a Intent)
        -> BoxFuture<'a, Result<Draft, ModelError>>;
}

// Permissions are intentionally NOT a MindState field and NOT writable by models.
// Existing Runtime owns a separate authority, single-writer actor, outbox and budgets.
// Commit only when relevant epochs still match and the intent remains authorized.
// Work results re-enter the EventBus; concurrent Workers cannot mutate MindState.

#[derive(Clone, Debug)]
pub enum WorkActivation {
    Immediate,
    Schedule { schedule: String },
    Event { event_filter: String },
    Condition { predicate: String },
}
#[derive(Clone, Debug)]
pub struct ObjectivePackage {
    pub objective: String,
    pub experience_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub activation: WorkActivation,
    pub capability_scope: String,
    pub budget: u64,
}
// Route a Reflect/ProposeWork intent to existing `work.create` after authorization.
// Keep trigger/wake implementation inside the runtime; no new LLM-facing facade.
