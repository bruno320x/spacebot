#!/usr/bin/env python3
"""
Unit tests for spacebot-acp-worker (the Intent bridge ACP worker), contract v2.1.

The fake intentd mirrors the LIVE shapes verified against intentd 0.9.38 on
2026-09-14 (docs/design-docs/acp-intent-bridge-result-path-2026-09-11.md §10.1):

  - event.query RPC result: {"events": [...]} with `type` (not event_type),
    `data` as an object (not data_json string), ISO timestamps, newest first.
  - agent.get result: {"agent": {...}} — status/lastMessageRole are snake-ish
    camelCase fields; lastAssistantPreview may be null (do not rely on it).
  - Full assistant text lives ONLY in the agent_message SQLite table
    (content = JSON block array) — read directly, read-only.
  - `agent:message` events carry role+turnId but NOT the text; the bridge
    must fetch the message body itself.

Run:  python3 test_spacebot_acp_worker.py
"""

import importlib.util
from importlib.machinery import SourceFileLoader
import json
import os
import socket
import sqlite3
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


def ev(event_id, event_type, data, agent_id="agent-1", ts="2026-09-14T07:00:00Z"):
    """Build an event as intentd's event.query RPC returns it (live shape)."""
    return {
        "id": event_id,
        "timestamp": ts,
        "type": event_type,
        "workspaceId": "spacebot-ile",
        "actor": {"type": "agent", "id": agent_id},
        "data": {"agentId": agent_id, **data},
    }


class FakeIntentd:
    """Minimal intentd stand-in with the live-verified RPC shapes."""

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
        resp = {"jsonrpc": "2.0", "id": req.get("id")}
        resp.update(self._respond(req))
        conn.sendall((json.dumps(resp) + "\n").encode("utf-8"))

    def _respond(self, req):
        method = req.get("method")
        if method == "agent.list":
            return {"result": {"agents": [
                {"id": "agent-1", "name": "Coordinator",
                 "metadata": {"isInitialAgent": True}},
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
            ordered = sorted(self.events, key=lambda e: e.get("timestamp", ""), reverse=True)
            return {"result": ordered}  # LIVE shape: bare list
        return {"error": {"code": -32601, "message": f"unknown method {method}"}}

    def close(self):
        self._stop.set()
        self._thread.join(timeout=2)
        self._srv.close()


def seed_message_db(socket_path, message_id, text):
    """Create a real agent_message DB next to the fake socket, exactly like
    the live one, so the bridge's direct read-only sqlite read works."""
    db = os.path.join(os.path.dirname(socket_path), "intentd.db")
    conn = sqlite3.connect(db)
    conn.execute(
        "CREATE TABLE agent_message (id TEXT PRIMARY KEY, agent_id TEXT, "
        "seq INTEGER, role TEXT, content TEXT, created_at TEXT)"
    )
    conn.execute(
        "INSERT INTO agent_message (id, agent_id, seq, role, content, created_at) "
        "VALUES (?,?,?,?,?,?)",
        (message_id, "agent-1", 2, "assistant",
         json.dumps([{"type": "text", "id": f"{message_id}:0", "text": text}]),
         "2026-09-14T07:00:01Z"),
    )
    conn.commit()
    conn.close()


class BridgeResultPathTest(unittest.TestCase):
    def setUp(self):
        self.mod = load_worker()
        self.updates = []
        self.mod.write_msg = lambda msg: self.updates.append(msg)
        self.mod.POLL_SECS = 0.05
        self.mod.RESULT_BUDGET_SECS = 5.0
        self.mod.HEARTBEAT_SECS = 3600.0

    def tearDown(self):
        if getattr(self, "fake", None):
            self.fake.close()

    def _start_fake(self, seed_db_for=None, **kwargs):
        self.fake = FakeIntentd(**kwargs)
        self.mod.INTENT_SOCKET = self.fake.path
        self.mod.INTENT_DB_PATH = os.path.join(os.path.dirname(self.fake.path), "intentd.db")
        if seed_db_for:
            seed_message_db(self.fake.path, seed_db_for[0], seed_db_for[1])
        return self.fake

    # -- 1. the real result comes back (event -> sqlite full text) ----------
    def test_returns_real_final_text(self):
        self._start_fake(
            seed_db_for=("msg-1", "4"),
            events=[
                ev(1, "agent:stream:status", {"message": "reading files"}),
                ev(2, "agent:message", {"role": "assistant", "messageId": "msg-1",
                                        "turnId": "turn-1"}),
            ],
        )
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
            seed_db_for=("msg-other", "wrong answer"),
            events=[
                ev(1, "agent:message", {"role": "assistant", "messageId": "msg-other",
                                        "turnId": "turn-1"}, agent_id="other-agent"),
            ],
            agent_state={"status": "idle", "lastMessageRole": "assistant"},
        )
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-4",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertIn("error", resp, "other agents' completions must not finish OUR turn")

    # -- 5. lastAgentResponse fallback (live: it is the trailing chunk) ------
    def test_last_agent_response_fallback_when_db_read_fails(self):
        self._start_fake(
            agent_state={"status": "idle", "lastMessageRole": "assistant"},
            events=[
                ev(2, "agent:last-message", {"role": "assistant", "turnId": "turn-1",
                                             "lastAgentResponse": "fallback text"}),
            ],
        )
        # No DB seeded -> sqlite read fails -> fall back to the event field.
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-5",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "fallback text")

    # -- 6. agent.get failure is non-fatal ------------------------------------
    def test_agent_get_failure_does_not_break_event_completion(self):
        self._start_fake(
            fail_agent_get=True,
            seed_db_for=("msg-6", "from events"),
            events=[ev(1, "agent:message", {"role": "assistant", "messageId": "msg-6",
                                            "turnId": "turn-1"})],
        )
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-6",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertEqual(resp["result"]["text"], "from events")

    # -- 7. progress/heartbeats never pollute the accumulated result ----------
    def test_progress_and_heartbeats_are_not_text(self):
        self._start_fake(
            seed_db_for=("msg-7", "clean result"),
            events=[
                ev(1, "agent:stream:status", {"message": "step one"}),
                ev(2, "agent:stream:status", {"message": "step two"}),
                ev(3, "agent:message", {"role": "assistant", "messageId": "msg-7",
                                        "turnId": "turn-1"}),
            ],
        )
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

    # -- 8. non-text assistant messages (tool blocks) are skipped -------------
    # -- 9. agent selection prefers the name "Coordinator" over the stale flag
    def test_agent_selection_prefers_coordinator_name(self):
        self._start_fake(events=[])
        # Live bug: a renamed old agent still carries isInitialAgent; the
        # bridge must pick the agent NAMED Coordinator.
        self.mod.INTENT_SOCKET = self.fake.path
        agents = [
            {"id": "agent-old", "name": "pi-eski", "metadata": {"isInitialAgent": True}},
            {"id": "agent-1", "name": "Coordinator"},
        ]
        self.fake._respond = lambda req: ({"result": {"agents": agents}}
                                          if req.get("method") == "agent.list"
                                          else FakeIntentd._respond(self.fake, req))
        agent_id, name = self.mod.find_target_agent("spacebot-ile")
        self.assertEqual((agent_id, name), ("agent-1", "Coordinator"))

    def test_textless_assistant_message_is_skipped(self):
        self._start_fake(
            # DB row has NO text block -> bridge must not finish on it
            events=[
                ev(1, "agent:message", {"role": "assistant", "messageId": "msg-placeholder",
                                        "turnId": "turn-1"}),
            ],
        )
        # Seed a message whose content has NO text blocks (tool block only):
        seed_message_db(self.fake.path, "msg-placeholder", "placeholder")
        db = os.path.join(os.path.dirname(self.fake.path), "intentd.db")
        conn = sqlite3.connect(db)
        conn.execute("UPDATE agent_message SET content = ? WHERE id = ?",
                     (json.dumps([{"type": "tool_use", "id": "t1", "name": "shell"}]),
                      "msg-placeholder"))
        conn.commit()
        conn.close()

        self.mod.RESULT_BUDGET_SECS = 0.3
        resp = self.mod.handle_session_prompt(1, {
            "sessionId": "sess-8",
            "prompt": [{"type": "text", "text": "task"}],
        })
        self.assertIn("error", resp, "a textless message must not be the result")


class BridgeFailurePathTest(unittest.TestCase):
    """Old-contract guards: the routing ack must never masquerade as a result."""

    def setUp(self):
        self.mod = load_worker()
        self.updates = []
        self.mod.write_msg = lambda msg: self.updates.append(msg)
        self.mod.POLL_SECS = 0.05

    def test_send_message_rpc_error_is_error(self):
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
