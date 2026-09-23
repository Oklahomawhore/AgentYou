from __future__ import annotations
import asyncio
from dataclasses import asdict, replace
from datetime import datetime
import json
from pathlib import Path
import tempfile
import unittest
from zoneinfo import ZoneInfo
from persona_agent.engine import Engine
from persona_agent.policy import decide, update_affect
from persona_agent.providers import MockJev, MockDialogue, MemorySink, parse_response
from persona_agent.store import Store
from persona_agent.types import Affect, Assessment, Candidate, Draft, Event, Persona

class RuntimeTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = str(Path(self.tmp.name) / "mind.sqlite")
        self.now = datetime(2026, 9, 22, 12, tzinfo=ZoneInfo("Asia/Tokyo")).timestamp()
        self.persona = Persona()
        self.store = Store(self.path, self.persona, self.now)
        self.jev, self.llm, self.sink = MockJev(), MockDialogue(), MemorySink()
        self.engine = Engine(self.store, self.persona, self.jev, self.llm, self.sink, lambda: self.now)
        await self.engine.set_guards(proactive_consent=True)

    async def asyncTearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def event(self, id_: str = "e1", candidate_id: str | None = None) -> Event:
        c = Candidate(candidate_id or id_, "video", "渲染已完成。", "此前约定告知", (id_,))
        return Event(id_, "work", "work_result", "Render finished; user asked for a completion notice.", self.now, self.now + 300, c)

    async def test_success(self):
        result = await self.engine.handle(self.event())
        self.assertEqual(result.status, "SENT")
        self.assertEqual(self.jev.calls, 1)
        self.assertEqual(self.llm.calls, 1)
        self.assertEqual(len(self.sink.messages), 1)
        self.assertEqual(self.store.rows()[0]["status"], "SENT")

    async def test_duplicate_event_is_not_reappraised(self):
        e = self.event()
        await self.engine.handle(e)
        affect = self.store.affect()
        result = await self.engine.handle(e)
        self.assertEqual(result.status, "DUPLICATE")
        self.assertEqual(self.jev.calls, 1)
        self.assertEqual(self.store.affect(), affect)

    async def test_no_consent_blocks_before_model(self):
        await self.engine.set_guards(proactive_consent=False)
        result = await self.engine.handle(self.event())
        self.assertEqual(result.reason, "proactive_not_opted_in")
        self.assertEqual(self.jev.calls, 0)

    async def test_quiet_blocks_before_model(self):
        await self.engine.set_guards(quiet=True)
        result = await self.engine.handle(self.event())
        self.assertEqual(result.reason, "quiet_or_busy")
        self.assertEqual(self.jev.calls, 0)

    async def test_direct_bypasses_proactive_gate_and_jev(self):
        await self.engine.set_guards(proactive_consent=False, quiet=True, busy=True, daily_limit=0)
        e = Event("u1", "user", "user_message", "你好", self.now, self.now + 60)
        self.assertEqual((await self.engine.handle(e)).status, "SENT")
        self.assertEqual(self.jev.calls, 0)
        self.assertEqual(self.store.budget(self.now)[0], 0)

    async def test_pause_is_a_hard_stop_even_for_direct(self):
        await self.engine.set_guards(paused=True)
        e = Event("u1", "user", "user_message", "你好", self.now, self.now + 60)
        self.assertEqual((await self.engine.handle(e)).reason, "paused")
        self.assertEqual(self.llm.calls, 0)

    async def test_unknown_source_is_not_persisted(self):
        e = replace(self.event(), source="secret-mailbox")
        self.assertEqual((await self.engine.handle(e)).reason, "source_not_authorized")
        self.assertEqual(self.store.recent(), [])

    async def test_event_cannot_claim_to_be_user(self):
        e = replace(self.event(), kind="user_message", source="browser")
        with self.assertRaises(ValueError):
            await self.engine.handle(e)

    async def test_cloud_denied_before_remote_call(self):
        self.jev.remote = True
        self.assertEqual((await self.engine.handle(self.event())).reason, "cloud_not_authorized")
        self.assertEqual(self.jev.calls, 0)

    async def test_failure_is_passive(self):
        self.jev.fail = True
        self.assertEqual((await self.engine.handle(self.event())).reason, "decision_unavailable")
        self.assertEqual(self.llm.calls, 0)

    async def test_low_evidence_requests_reflection_without_speaking(self):
        self.jev.assessment = replace(self.jev.assessment, evidence_sufficient=.4)
        self.assertEqual((await self.engine.handle(self.event())).status, "REFLECT")
        self.assertEqual(self.llm.calls, 0)
        self.assertEqual(len(self.sink.messages), 0)
        row = self.store.db.execute("SELECT kind,payload FROM thoughts").fetchone()
        self.assertEqual(row["kind"], "reflection_request")
        self.assertEqual(json.loads(row["payload"])["status"], "pending")

    async def test_duplicate_candidate_is_not_resent(self):
        await self.engine.set_guards(cooldown_seconds=0)
        await self.engine.handle(self.event("e1", "same-result"))
        result = await self.engine.handle(self.event("e2", "same-result"))
        self.assertEqual(result.status, "DUPLICATE")
        self.assertEqual(len(self.sink.messages), 1)

    async def test_expired_event_is_not_evaluated(self):
        e = self.event()
        self.now += 301
        self.assertEqual((await self.engine.handle(e)).reason, "expired_or_future_event")
        self.assertEqual(self.jev.calls, 0)

    async def test_unknown_evidence_prevents_generation(self):
        e = self.event()
        e = replace(e, candidate=replace(e.candidate, evidence_ids=("missing",)))
        self.assertEqual((await self.engine.handle(e)).reason, "unknown_evidence_ids")
        self.assertEqual(self.llm.calls, 0)

    async def test_cooldown(self):
        await self.engine.handle(self.event("e1"))
        self.now += 60
        self.assertEqual((await self.engine.handle(self.event("e2"))).reason, "cooldown")
        self.assertEqual(self.jev.calls, 1)

    async def test_local_day_budget_rollover(self):
        await self.engine.set_guards(daily_limit=1, cooldown_seconds=0)
        await self.engine.handle(self.event("e1"))
        self.assertEqual((await self.engine.handle(self.event("e2"))).reason, "daily_budget")
        self.now += 86400
        self.assertEqual((await self.engine.handle(self.event("e3"))).status, "SENT")

    async def test_changed_guards_discard_inflight_draft(self):
        async def callback():
            await self.engine.set_guards(quiet=True)
        self.llm.before_return = callback
        self.assertEqual((await self.engine.handle(self.event())).status, "STALE")
        self.assertEqual(len(self.sink.messages), 0)

    async def test_event_during_draft_invalidates_old_snapshot(self):
        async def callback():
            self.store.ingest(Event("context-change", "browser", "observation", "User changed focus.", self.now, self.now + 60))
        self.llm.before_return = callback
        self.assertEqual((await self.engine.handle(self.event())).status, "STALE")
        self.assertEqual(len(self.sink.messages), 0)

    async def test_expiry_during_generation_is_rechecked(self):
        async def callback():
            self.now += 301
        self.llm.before_return = callback
        self.assertEqual((await self.engine.handle(self.event())).reason, "expired_or_future_event")
        self.assertEqual(len(self.sink.messages), 0)

    async def test_ambiguous_delivery_remains_unknown_without_retry(self):
        self.sink.fail = True
        e = self.event()
        self.assertEqual((await self.engine.handle(e)).status, "UNKNOWN")
        self.assertEqual(self.store.rows()[0]["status"], "UNKNOWN")
        self.assertEqual((await self.engine.handle(e)).status, "DUPLICATE")
        self.assertEqual(self.sink.calls, 1)
        self.assertEqual(self.store.budget(self.now)[0], 1)

    async def test_missing_receipt_recovered_as_unknown_after_restart(self):
        result = await self.engine.handle(self.event())
        self.store.db.execute("UPDATE outbox SET status='SENDING' WHERE id=?", (result.outbox_id,))
        affect = self.store.affect()
        self.store.close()
        self.store = Store(self.path, self.persona, self.now)
        self.assertEqual(self.store.rows()[0]["status"], "UNKNOWN")
        self.assertEqual(self.store.affect(), affect)

    async def test_current_observation_not_mistaken_for_previous_context(self):
        e = self.event()
        self.store.ingest(e)
        state = json.loads(self.engine._snapshot(e).state_json)
        self.assertEqual(state["recent_events"], [])
        self.assertEqual(state["evidence"][0]["event_id"], e.event_id)

class PureTests(unittest.TestCase):
    def test_affect_bounded_and_decays(self):
        a = Assessment(1,1,0,1,1,1,1,1)
        affect = Affect(updated_at=100)
        for i in range(1000):
            affect = update_affect(affect, a, 101+i)
        self.assertTrue(-1 <= affect.valence <= 1)
        self.assertTrue(0 <= affect.arousal <= 1)
        self.assertTrue(0 <= affect.control <= 1)
        neutral = replace(a, goal_congruence=.5, surprise=0, evidence_sufficient=.5)
        relaxed = update_affect(affect, neutral, 100000)
        self.assertLess(abs(relaxed.valence), abs(affect.valence))

    def test_confidence_not_probability_of_correctness(self):
        # Changing the provider's distribution statistic cannot bypass semantic gates.
        a = Assessment(.1,.9,.1,.9,.5,.5,.99,1)
        self.assertEqual(decide(a, Affect()).action, "WAIT")

    def response(self):
        scalars = {key: {"type": "noul", "noul": .9} for key in
                   ("benefit","novelty","interruption","interest","goal_congruence","surprise")}
        scalars["evidence"] = {"type":"choice", "choice":"sufficient", "probabilities":
                              {"sufficient":.9,"missing":.08,"conflict":.02}, "confidence":.8}
        return {"model":"jev-1.13.0", "answers":scalars}

    def test_rest_parser_uses_noul_field(self):
        self.assertEqual(parse_response(self.response()).benefit, .9)

    def test_rest_parser_rejects_nan_and_bool(self):
        for value in (float("nan"), True, -1, 2):
            r = self.response()
            r["answers"]["benefit"]["noul"] = value
            with self.assertRaises(ValueError):
                parse_response(r)

    def test_rest_parser_requires_normalized_choice(self):
        r = self.response()
        r["answers"]["evidence"]["probabilities"]["missing"] = .5
        with self.assertRaises(ValueError):
            parse_response(r)

    def test_rest_parser_rejects_choice_probability_mismatch(self):
        r = self.response()
        r["answers"]["evidence"]["choice"] = "missing"
        with self.assertRaises(ValueError):
            parse_response(r)

    def test_affect_changes_marginal_behavior_but_not_hard_constraints(self):
        a = Assessment(.84,.70,.08,.61,.8,.5,.95,.8)
        cold = decide(a, Affect(-1, 0, .5))
        warm = decide(a, Affect(1, 1, .5))
        self.assertNotEqual(cold.action, warm.action)
        self.assertEqual(decide(replace(a, interruption=.9), Affect(1,1,.5)).action, "WAIT")

if __name__ == "__main__":
    unittest.main()
