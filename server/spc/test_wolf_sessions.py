"""Tests for wolf_sessions.py. Run from server/spc: python3 -m unittest -v test_wolf_sessions"""

import io
import json
import os
import queue
import socketserver
import tempfile
import threading
import unittest

from wolf_sessions import Tracker, event_kind, request, session_id_of, sse_events, watch

SERVERS = ["nvidia", "amdgpu"]
TV = "5334278758390175681"


class TrackerTest(unittest.TestCase):
    def setUp(self):
        self.t = Tracker(SERVERS, idle_seconds=1800)

    def test_pause_ends_after_the_idle_time_once(self):
        self.t.on_event("amdgpu", "pause", TV, now=0)
        self.assertEqual(self.t.due(now=1799), [])
        self.assertEqual(self.t.due(now=1800), [("amdgpu", TV)])
        self.assertEqual(self.t.due(now=5000), [])

    def test_resume_cancels(self):
        self.t.on_event("amdgpu", "pause", TV, now=0)
        self.t.on_event("amdgpu", "resume", TV, now=10)
        self.assertEqual(self.t.due(now=9999), [])

    def test_stop_cancels(self):
        self.t.on_event("nvidia", "pause", TV, now=0)
        self.t.on_event("nvidia", "stop", TV, now=5)
        self.assertEqual(self.t.due(now=9999), [])

    def test_start_or_resume_ends_the_other_servers_session(self):
        self.assertEqual(self.t.on_event("amdgpu", "start", TV, now=0), [("nvidia", TV)])
        self.assertEqual(self.t.on_event("nvidia", "resume", TV, now=1), [("amdgpu", TV)])
        self.assertEqual(self.t.on_event("nvidia", "pause", TV, now=2), [])

    def test_disconnect_forgets_that_servers_pauses(self):
        self.t.on_event("nvidia", "pause", TV, now=0)
        self.t.on_event("amdgpu", "pause", "42", now=0)
        self.t.on_disconnect("nvidia")
        self.assertEqual(self.t.due(now=9999), [("amdgpu", "42")])


class ParsingTest(unittest.TestCase):
    def test_event_kind(self):
        self.assertEqual(event_kind("wolf::core::events::StreamSession"), "start")
        self.assertEqual(event_kind("wolf::core::events::PauseStreamEvent"), "pause")
        self.assertEqual(event_kind("wolf::core::events::ResumeStreamEvent"), "resume")
        self.assertEqual(event_kind("wolf::core::events::StopStreamEvent"), "stop")
        self.assertIsNone(event_kind("wolf::core::events::VideoSession"))
        self.assertIsNone(event_kind("PairSignal"))

    def test_session_id_of(self):
        self.assertEqual(session_id_of('{"session_id": 5334278758390175681}'), TV)
        self.assertEqual(session_id_of('{"session_id": "42"}'), "42")
        self.assertIsNone(session_id_of('{"other": 1}'))
        self.assertIsNone(session_id_of("not json"))
        self.assertIsNone(session_id_of("[1, 2]"))

    def test_sse_events(self):
        stream = io.BytesIO(
            b'event: A\ndata: {"x": 1}\n\n'
            b": comment\n"
            b"event: B\r\ndata: 2\r\n\r\n"
            b"event: C\ndata: unterminated"
        )
        self.assertEqual(list(sse_events(stream)), [("A", '{"x": 1}'), ("B", "2")])


class FakeWolf:
    """A Unix-socket server answering like Wolf's API; records request bodies."""

    def __init__(self, reply):
        self.dir = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.dir.name, "wolf.sock")
        self.bodies = []
        fake = self

        class Handler(socketserver.StreamRequestHandler):
            timeout = 2  # a request missing its body fails the test, not hangs it

            def handle(self):
                self.rfile.readline()
                length = 0
                while (line := self.rfile.readline()) not in (b"\r\n", b""):
                    name, _, value = line.decode().partition(":")
                    if name.lower() == "content-length":
                        length = int(value)
                fake.bodies.append(self.rfile.read(length).decode())
                self.wfile.write(reply)

        self.server = socketserver.UnixStreamServer(self.path, Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.dir.cleanup()


class SocketTest(unittest.TestCase):
    def test_request_sends_the_body_and_reads_the_reply(self):
        wolf = FakeWolf(b'HTTP/1.0 200 OK\r\nContent-Length: 16\r\n\r\n{"success":true}')
        self.addCleanup(wolf.close)
        body = json.dumps({"session_id": TV}).encode()
        self.assertEqual(request(wolf.path, "POST", "/api/v1/sessions/stop", body), (200, '{"success":true}'))
        self.assertEqual(json.loads(wolf.bodies[0]), {"session_id": TV})

    def test_request_reports_an_error_status(self):
        wolf = FakeWolf(b"HTTP/1.0 500 OK\r\nContent-Length: 9\r\n\r\nnot found")
        self.addCleanup(wolf.close)
        self.assertEqual(request(wolf.path, "POST", "/api/v1/sessions/stop", b"{}"), (500, "not found"))

    def test_watch_reports_session_events_then_a_disconnect(self):
        wolf = FakeWolf(
            b"HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\n\r\n"
            b'event: wolf::core::events::VideoSession\ndata: {"session_id": 1}\n\n'
            b"event: wolf::core::events::PauseStreamEvent\ndata: {\"session_id\": " + TV.encode() + b"}\n\n"
        )
        self.addCleanup(wolf.close)
        events = queue.Queue()
        threading.Thread(target=watch, args=("nvidia", wolf.path, events), daemon=True).start()
        self.assertEqual(events.get(timeout=5), ("nvidia", "pause", TV))
        self.assertEqual(events.get(timeout=5), ("nvidia", "disconnect", None))
        # It reconnects on its own and reads the stream again.
        self.assertEqual(events.get(timeout=5), ("nvidia", "pause", TV))


if __name__ == "__main__":
    unittest.main()
