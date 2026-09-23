use crate::model::*;

pub fn hard_gate(
    event: &Event,
    guards: &Guards,
    now: i64,
    count: u32,
    last: Option<i64>,
) -> Option<&'static str> {
    if guards.paused && !guards.ai_driven {
        return Some("paused");
    }
    if !guards.allowed_sources.contains(&event.source) {
        return Some("source_not_authorized");
    }
    if now < event.created_at_ms.saturating_sub(5000) || now >= event.expires_at_ms {
        return Some("expired_or_future_event");
    }
    if event.kind == EventKind::UserMessage {
        return None;
    }
    let Some(c) = &event.candidate else {
        return Some("no_candidate");
    };
    if guards.ai_driven {
        return None;
    }
    let Some(p) = guards.kinds.iter().find(|p| p.kind == c.kind) else {
        return Some("kind_not_authorized");
    };
    if !p.consent {
        return Some("kind_not_authorized");
    }
    if guards.quiet || guards.busy {
        return Some("quiet_or_busy");
    }
    if count >= p.daily_limit {
        return Some("daily_budget");
    }
    if last.is_some_and(|t| now.saturating_sub(t) < p.cooldown_ms) {
        return Some("cooldown");
    }
    None
}
pub fn reduce(old: Affect, a: &Appraisal, now: i64) -> Affect {
    let dt = now.saturating_sub(old.updated_at_ms).clamp(0, 86_400_000) as f64 / 1000.;
    let decay = (-dt / 3600.).exp();
    Affect {
        valence: (old.valence * decay + 0.15 * (2. * a.goal_congruence - 1.)).clamp(-1., 1.),
        arousal: (0.2 + (old.arousal - 0.2) * decay + 0.12 * a.surprise).clamp(0., 1.),
        control: (0.5 + (old.control - 0.5) * decay + 0.06 * (2. * a.evidence_sufficient - 1.))
            .clamp(0., 1.),
        updated_at_ms: old.updated_at_ms.max(now),
    }
}
pub fn decide(event: &Event, a: &Appraisal, affect: Affect) -> Decision {
    if let Some(action) = a.selected_action {
        return Decision::new(event, action, "jev_action");
    }
    if a.evidence_sufficient < 0.8 {
        return Decision::new(
            event,
            if a.benefit >= 0.8 {
                Action::Reflect
            } else {
                Action::Wait
            },
            "evidence_not_ready",
        );
    }
    if a.benefit < 0.8 || a.novelty < 0.65 || a.interruption > 0.2 {
        return Decision::new(event, Action::Wait, "semantic_gate");
    }
    let utility = 0.55 * a.benefit + 0.25 * a.novelty + 0.2 * a.interest - 0.8 * a.interruption
        + 0.04 * affect.valence
        + 0.02 * affect.arousal;
    let mut d = Decision::new(
        event,
        if utility >= 0.72 {
            Action::Speak
        } else {
            Action::Wait
        },
        "weighted_appraisal",
    );
    d.utility = utility;
    d
}
