from __future__ import annotations
import argparse
import asyncio
from dataclasses import asdict
import json
import tempfile
import time
from pathlib import Path
from .engine import Engine
from .providers import JevHTTP, MockDialogue, MockJev, MemorySink
from .store import Store
from .types import Candidate, Event, Persona

async def main(live_jev: bool):
    persona = Persona()
    now = time.time()
    # Disposable, synthetic content only. Never inspect the real desktop in this demo.
    with tempfile.TemporaryDirectory() as tmp:
        store = Store(str(Path(tmp) / "mind.sqlite"), persona, now)
        sink = MemorySink()
        engine = Engine(store, persona, JevHTTP() if live_jev else MockJev(), MockDialogue(), sink)
        await engine.set_guards(proactive_consent=True, cloud_allowed=live_jev)
        candidate = Candidate("render-42-complete", "video-editing", "你刚才等的视频渲染完成了，结果已经通过媒体校验。",
                              "用户明确要求完成后告知", ("work-42",))
        event = Event("work-42", "work", "work_result", "Render job 42 succeeded; validation passed. User requested completion notice.",
                      now, now + 120, candidate)
        print("mode:", "LIVE Jev / MOCK dialogue" if live_jev else "OFFLINE MOCKS")
        print("first:", asdict(await engine.handle(event)))
        print("duplicate:", asdict(await engine.handle(event)))
        await engine.set_guards(quiet=True)
        user = Event("user-1", "user", "user_message", "把结果给我看看。", time.time(), time.time() + 120)
        print("direct user message despite quiet hours:", asdict(await engine.handle(user)))
        print("affect:", asdict(store.affect()))
        print("messages:", json.dumps(sink.messages, ensure_ascii=False, indent=2))
        store.close()

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--live-jev", action="store_true", help="Send ONLY synthetic demo state to Jev; requires TYPESAFE_API_KEY")
    asyncio.run(main(parser.parse_args().live_jev))
