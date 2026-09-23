"""Example policy, NOT a calibrated production threshold set or a neuroscience model."""
from __future__ import annotations
from dataclasses import replace
import math
from .types import Affect, Assessment, Decision, Event, Guards

POLICY_VERSION = "reference-1"
QUESTION_VERSION = "atomic-appraisal-1"

def hard_gate(event: Event, guards: Guards, now: float, count: int = 0,
              last_attempt: float | None = None) -> str | None:
    if guards.paused:
        return "paused"
    if event.source not in guards.allowed_sources:
        return "source_not_authorized"
    if now < event.created_at - 5 or now >= event.expires_at:
        return "expired_or_future_event"
    # A real user turn is not a proactive interruption.
    if event.kind == "user_message":
        return None
    if not guards.proactive_consent:
        return "proactive_not_opted_in"
    if guards.quiet or guards.busy:
        return "quiet_or_busy"
    if event.candidate is None:
        return "no_candidate"
    if count >= guards.daily_limit:
        return "daily_budget"
    if last_attempt is not None and now - last_attempt < guards.cooldown_seconds:
        return "cooldown"
    return None

def update_affect(old: Affect, a: Assessment, now: float) -> Affect:
    """Bounded appraisal + decay. The output represents simulated affect only."""
    a.validate()
    # Never integrate negative time, or pretend an arbitrarily long suspension was activity.
    dt = max(0.0, min(now - old.updated_at, 86400.0))
    decay = math.exp(-dt / 3600.0)
    valence = old.valence * decay + 0.15 * (2 * a.goal_congruence - 1)
    arousal = 0.2 + (old.arousal - 0.2) * decay + 0.12 * a.surprise
    control = 0.5 + (old.control - 0.5) * decay + 0.06 * (2 * a.evidence_sufficient - 1)
    return Affect(max(-1.0, min(1.0, valence)), max(0.0, min(1.0, arousal)),
                  max(0.0, min(1.0, control)), max(old.updated_at, now))

def decide(a: Assessment, affect: Affect) -> Decision:
    a.validate()
    if a.evidence_sufficient < 0.8:
        return Decision("REFLECT" if a.benefit >= 0.8 else "WAIT", "evidence_not_ready")
    if a.benefit < 0.8 or a.novelty < 0.65 or a.interruption > 0.2:
        return Decision("WAIT", "semantic_gate")
    # Deliberately do NOT multiply probabilities, nor use confidence as accuracy.
    utility = (0.55 * a.benefit + 0.25 * a.novelty + 0.20 * a.interest
               - 0.80 * a.interruption + 0.04 * affect.valence + 0.02 * affect.arousal)
    return Decision("SPEAK" if utility >= 0.72 else "WAIT", "weighted_appraisal", utility)
