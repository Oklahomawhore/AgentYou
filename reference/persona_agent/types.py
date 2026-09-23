"""Reference data contracts. All permission fields must come from the trusted host."""
from __future__ import annotations
from dataclasses import asdict, dataclass
import json
import math

@dataclass(frozen=True)
class Persona:
    persona_id: str = "original-digital-researcher"
    version: str = "1"
    name: str = "知微"
    values: tuple[str, ...] = ("求证", "独立判断", "尊重对方的注意力")
    interests: tuple[str, ...] = ("智能体", "视频创作", "计算机系统")
    identity: str = "原创数字角色，仅通过授权的电脑环境获得经历，不是真实人物。"

@dataclass(frozen=True)
class Affect:
    valence: float = 0.0
    arousal: float = 0.2
    control: float = 0.5
    updated_at: float = 0.0

@dataclass(frozen=True)
class Guards:
    proactive_consent: bool = False
    cloud_allowed: bool = False
    paused: bool = False
    quiet: bool = False
    busy: bool = False
    daily_limit: int = 3
    cooldown_seconds: float = 1800.0
    timezone: str = "Asia/Tokyo"
    allowed_sources: tuple[str, ...] = ("user", "work", "browser")

@dataclass(frozen=True)
class Candidate:
    candidate_id: str
    topic: str
    proposition: str
    reason: str
    evidence_ids: tuple[str, ...]

@dataclass(frozen=True)
class Event:
    event_id: str
    source: str
    kind: str
    text: str
    created_at: float
    expires_at: float
    candidate: Candidate | None = None

    def validate(self) -> None:
        if not self.event_id or len(self.event_id) > 128:
            raise ValueError("invalid event_id")
        if not self.source or self.kind not in {"user_message", "observation", "work_result", "wake"}:
            raise ValueError("invalid event source/kind")
        if self.kind == "user_message" and self.source != "user":
            raise ValueError("only a trusted user adapter can create user_message")
        if not all(math.isfinite(t) for t in (self.created_at, self.expires_at)):
            raise ValueError("invalid timestamps")
        if self.expires_at <= self.created_at or len(self.text) > 16000:
            raise ValueError("invalid expiry or text too long")
        if self.candidate:
            c = self.candidate
            if not c.candidate_id or not c.evidence_ids or len(c.proposition) > 4000:
                raise ValueError("invalid candidate; explicit evidence required")

@dataclass(frozen=True)
class Snapshot:
    revision: int
    event: Event
    persona: Persona
    affect: Affect
    # Serialized JSON avoids passing mutable shared dictionaries to providers.
    state_json: str

@dataclass(frozen=True)
class Assessment:
    benefit: float
    novelty: float
    interruption: float
    interest: float
    goal_congruence: float
    surprise: float
    evidence_sufficient: float
    evidence_confidence: float  # API distribution statistic, NOT probability of correctness.
    model: str = "mock"

    def validate(self) -> None:
        for key, value in asdict(self).items():
            if key == "model":
                continue
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or not 0 <= value <= 1:
                raise ValueError(f"invalid probability/statistic: {key}")

@dataclass(frozen=True)
class Decision:
    action: str  # WAIT | REFLECT | SPEAK | RESPOND
    reason: str
    utility: float = 0.0

@dataclass(frozen=True)
class Draft:
    text: str
    evidence_ids: tuple[str, ...] = ()

@dataclass(frozen=True)
class Result:
    status: str
    reason: str
    outbox_id: str | None = None


def json_text(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
