#!/usr/bin/env python3
"""
Unit tests for spacebot-acp-worker (the Intent bridge ACP worker).

Runs the worker module against a fake intentd daemon over a real UNIX socket
and asserts the result-path contract from
docs/design-docs/acp-intent-bridge-result-path-2026-09-11.md:

  1. session/prompt returns the REAL final text, not the routing ack.
  2. Budget exhaustion is an explicit JSON-RPC error — never a silent
     "completed".
  3. A missing/unresponsive intentd is an error, not a done.
  4. Events from other agents are filtered by agentId.
  5. Empty stream:end text falls back to agent:idle lastResponseSummary.
  6. agent.get failures are non-fatal when the event stream completes.
  7. Progress/heartbeat updates never carry type "text" — only the final
     completed update does — so the parent's accumulation stays clean.

Run:  python3 test_spacebot_acp_worker.py
"""

import importlib.util
from importlib.machinery import SourceFileLoader
import json
import os
import socket
import tempfile
import threading
import unittest

WORKER_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "spacebot-acp-worker")


def load_worker():
    """Load the worker script as a fresh module instance per test."""
    loader = SourceFileLoader("spacebot_acp_worker_under_test", WORKER_PATH)
    spec = importlib.util.spec_from_loader(loader.name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


def ev(event_id, event_type, data, agent_id="agent-1"):
    """Build an event row as intentd's event.query returns it (data_json is a string)."""
    return {
        "id": event_id,
        "timestamp": 1000 + event_id,
        "event_type": event_type,
        "workspace_id": "spacebot-ile",
        "data_json": json.dumps({"agentId": agent_id, **data}),
    }


class FakeIntentd:
    """Minimal intentd stand-in: agent.list / agent.sendMessage / agent.get / event.query.

    Realism: `event.query` returns nothing until `agent.sendMessage` is called
    (a turn's events only exist after routing), newest-first like the real
    fixed ordering.
    """

    def __init__(self, events=None, agent_state=None, fail_agent_get=False):
        self.pending_events = list(events or [])
        self.events = []
        self.agent_state = agent_state or {"status": "busy"}
        self.fail_agent_get = fail_agent_get
        self.calls = []
        self._dir = tempfile.mkdtemp(prefix="fake-intentd-")
        self.path = os.path.join(self._dir, "intentd.sock")
        self._srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._srv.bind(self.path)
        self._srv.listen(16)
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        self._srv.settimeout(0.2)
        while not self._stop.is_set():
            try:
                conn, _ = self._srv.accept()
            except (socket.timeout, OSError):
                continue
            try:
                self._handle(conn)
            except Exception:
                pass
            finally:
                conn.close()

    def _handle(self, conn):
        f = conn.makefile("r", encoding="utf-8")
        line = f.readline()
        if not line:
            return
        try:
            req = json.loads(line)
        except Exception:
            return
        self.calls.append(req.get("method"))
        resp = self._respond(req)
        conn.sendall((json.dumps(resp) + "\n").encode("utf-8"))

    def _respond(self, req):
        method = req.get("method")
        if method == "agent.list":
            return {"result": {"agents": [
                {"id": "agent-1", "name": "Coordinator", "metadata": {"isInitialAgent": True}},
            ]}}
        if method == "agent.sendMessage":
            if not self.events:
                self.events = list(self.pending_events)
            return {"result": {"turnId": "turn-1"}}
        if method == "agent.get":
            if self.fail_agent_get:
                return {"error": {"code": -32000, "message": "boom"}}
            return {"result": {"agent": dict(self.agent_state)}}
        if method == "event.query":
            ordered = sorted(self.events, key=lambda e: e.get("timestamp", 0), reverse=True)
            return {"result": {"events": ordered}}
        return {"error": {"code": -32601, "message": f"unknown method {method}"}}

    def close(self):
        self._stop.set()
        self._thread.join(timeout=2)
        self._srv.close()


class BridgeResultPathTest(unittest.TestCase):
    def setUp(self):
        self.mod = load_worker()
        self.updates = []
        self.mod.write_msg = lambda msg: self.updates.append(msg)
        # Fast tests: tiny poll/budget unless the test overrides.
        self.mod.POLL_SECS = 0.05
        self.mod.RESULT_BUDGET_SECS = 5.0
        self.mod.HEARTBEAT_SECS = 3600.0

    def tearDown(self):
        if getattr(self, "fake", None):
            self.fake.close()

    def _start_fake(self, **kwargs):
        self.fake = FakeIntentd(**kwargs)
        self.mod.INTENT_SOCKET = self.fake.path
        return self.fake

    # -- 1. the real result comes back -------------------------------------
    def test_returns_real_final_text(self):
        self._start_fake(events=[
            ev(1, "agent:stream:status", {"message": "reading files"}),
            ev(2, "agent:stream:end", {"lastAgentResponse": "4"}),
        ])
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-1",
            "prompt": [{"type": "text", "text": "2+2 kac eder?"}],
        })
        self.assertNotIn("error", resp)
        self.assertEqual(resp["result"]["text"], "4")

        completed = [u for u in self.updates if u["params"].get("status") == "completed"]
        self.assertEqual(len(completed), 1, "exactly one completed update")
        self.assertEqual(completed[0]["params"]["content"]["text"], "4")

    # -- 2. budget exhaustion is an explicit error --------------------------
    def test_budget_exhausted_returns_error_never_completed(self):
        self._start_fake(events=[ev(1, "agent:stream:status", {"message": "grinding"})])
        self.mod.RESULT_BUDGET_SECS = 0.3
        self.mod.POLL_SECS = 0.1

        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-2",
            "prompt": [{"type": "text", "text": "long task"}],
        })
        self.assertIn("error", resp, "timeout must be an error, not a silent done")
        completed = [u for u in self.updates if u["params"].get("status") == "completed"]
        self.assertEqual(completed, [], "no completed update on timeout")
        errored = [u for u in self.updates if u["params"].get("status") == "error"]
        self.assertEqual(len(errored), 1)

    # -- 3. dead intentd is an error ----------------------------------------
    def test_missing_socket_is_error_not_done(self):
        self.mod.INTENT_SOCKET = "/nonexistent/intentd.sock"
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-3",
            "prompt": [{"type": "text", "text": "hello"}],
        })
        self.assertIn("error", resp)
        completed = [u for u in self.updates if u["params"].get("status") == "completed"]
        self.assertEqual(completed, [])

    # -- 4. agentId filter ----------------------------------------------------
    def test_ignores_other_agents_events(self):
        self._start_fake(
            events=[
                ev(1, "agent:stream:end", {"lastAgentResponse": "wrong answer"}, agent_id="other-agent"),
            ],
            agent_state={"status": "idle", "lastMessageRole": "assistant",
                         "lastAssistantPreview": "correct answer"},
        )
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-4",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "correct answer")

    # -- 5. empty stream:end falls back to idle summary ----------------------
    def test_empty_end_text_falls_back_to_idle_summary(self):
        self._start_fake(events=[
            ev(2, "agent:stream:end", {"lastAgentResponse": ""}),
            ev(1, "agent:idle", {"lastResponseSummary": "summary text"}),
        ])
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-5",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "summary text")

    # -- 6. agent.get failure is non-fatal ------------------------------------
    def test_agent_get_failure_does_not_break_event_completion(self):
        self._start_fake(
            fail_agent_get=True,
            events=[ev(1, "agent:stream:end", {"lastAgentResponse": "from events"})],
        )
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-6",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "from events")

    # -- 7. progress/heartbeats never pollute the accumulated result ----------
    def test_progress_and_heartbeats_are_not_text(self):
        self._start_fake(events=[
            ev(1, "agent:stream:status", {"message": "step one"}),
            ev(2, "agent:stream:status", {"message": "step two"}),
            ev(3, "agent:stream:end", {"lastAgentResponse": "clean result"}),
        ])
        self.mod.HEARTBEAT_SECS = 0.0  # force a heartbeat on every idle poll
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-7",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "clean result")

        for u in self.updates:
            status = u["params"].get("status")
            content = u["params"].get("content") or {}
            if status == "completed":
                self.assertEqual(content.get("type"), "text")
            else:
                self.assertNotEqual(
                    content.get("type"), "text",
                    f"non-final update with status={status} must not carry type 'text'",
                )
        heartbeat = [u for u in self.updates
                     if (u["params"].get("content") or {}).get("type") == "intent_heartbeat"]
        self.assertGreater(len(heartbeat), 0, "idle polls should emit heartbeats")


class BridgeFailurePathTest(unittest.TestCase):
    """Old-contract guards: the routing ack must never masquerade as a result."""

    def setUp(self):
        self.mod = load_worker()
        self.updates = []
        self.mod.write_msg = lambda msg: self.updates.append(msg)
        self.mod.POLL_SECS = 0.05

    def test_send_message_rpc_error_is_error(self):
        # Socket exists but sendMessage fails -> explicit error.
        class RejectingFake(FakeIntentd):
            def _respond(self, req):
                if req.get("method") == "agent.sendMessage":
                    return {"error": {"code": -32000, "message": "queue rejected"}}
                return super()._respond(req)

        self.fake = RejectingFake()
        self.mod.INTENT_SOCKET = self.fake.path
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-r",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertIn("error", resp)
        completed = [u for u in self.updates if u["params"].get("status") == "completed"]
        self.assertEqual(completed, [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
