#!/usr/bin/env python3
"""Ends idle Wolf sessions and keeps the shared Steam to one server at a time.

Both Wolf servers on spc (wolf@nvidia: "spc", wolf@amdgpu: "spc-igpu") share
each device's Steam home. This service watches their event streams:
- a session paused for WOLF_IDLE_MINUTES (default 30) is ended;
- a session starting or resuming on one server ends the same session (the same
  device) on the other.
Run by wolf-sessions.service; logs go to the journal.
"""

import json
import os
import queue
import socket
import threading
import time

SERVERS = {
    "nvidia": "/run/wolf/nvidia/sockets/wolf.sock",
    "amdgpu": "/run/wolf/amdgpu/sockets/wolf.sock",
}

KINDS = {
    "StreamSession": "start",
    "PauseStreamEvent": "pause",
    "ResumeStreamEvent": "resume",
    "StopStreamEvent": "stop",
}


def log(msg):
    print(msg, flush=True)


def event_kind(event_type):
    """'wolf::core::events::PauseStreamEvent' -> 'pause'; None for others."""
    return KINDS.get(event_type.rsplit("::", 1)[-1].strip())


def session_id_of(data):
    """The session_id in an event's JSON, as a string (they exceed 2^53)."""
    try:
        value = json.loads(data)
    except ValueError:
        return None
    if not isinstance(value, dict) or value.get("session_id") is None:
        return None
    return str(value["session_id"])


def sse_events(lines):
    """(event, data) pairs from a Server-Sent Events byte stream."""
    event, data = None, []
    for raw in lines:
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if not line:
            if event is not None:
                yield event, "\n".join(data)
            event, data = None, []
        elif line.startswith("event:"):
            event = line[len("event:"):].strip()
        elif line.startswith("data:"):
            data.append(line[len("data:"):].strip())


class Tracker:
    """Decides which sessions to end. No I/O; time is passed in (seconds)."""

    def __init__(self, servers, idle_seconds):
        self.servers = list(servers)
        self.idle = idle_seconds
        self.paused = {}  # (server, session_id) -> when it paused

    def on_event(self, server, kind, session_id, now):
        """Returns (server, session_id) pairs to end now."""
        key = (server, session_id)
        if kind == "pause":
            self.paused[key] = now
            return []
        self.paused.pop(key, None)
        if kind in ("start", "resume"):
            return [(other, session_id) for other in self.servers if other != server]
        return []

    def on_disconnect(self, server):
        """The server's event stream broke (e.g. Wolf restarted): its sessions are gone."""
        for key in [k for k in self.paused if k[0] == server]:
            del self.paused[key]

    def due(self, now):
        """Sessions paused for the idle time; each is returned once."""
        expired = [k for k, t in self.paused.items() if now - t >= self.idle]
        for key in expired:
            del self.paused[key]
        return expired


def request(sock_path, method, path, body=b""):
    """One HTTP request over Wolf's Unix socket; returns (status, body)."""
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(15)
        s.connect(sock_path)
        s.sendall(
            f"{method} {path} HTTP/1.0\r\nHost: localhost\r\n"
            f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n".encode()
            + body
        )
        f = s.makefile("rb")
        status = int(f.readline().split()[1])
        length = None
        while (line := f.readline()) not in (b"\r\n", b"\n", b""):
            name, _, value = line.decode("latin-1").partition(":")
            if name.strip().lower() == "content-length":
                length = int(value)
        reply = f.read(length) if length is not None else f.read()
        return status, reply.decode("utf-8", "replace")


def stop(server, session_id, why):
    try:
        status, body = request(
            SERVERS[server],
            "POST",
            "/api/v1/sessions/stop",
            json.dumps({"session_id": session_id}).encode(),
        )
    except (OSError, ValueError, IndexError) as e:
        log(f"{server}: couldn't end session {session_id}: {e}")
        return
    if status == 200 and '"success":true' in body.replace(" ", ""):
        log(f"{server}: ended session {session_id} ({why})")
    # Otherwise it wasn't running there, the usual case after a switch.


def watch(server, sock_path, events):
    """Feeds (server, kind, session_id) into `events`; reconnects forever."""
    delay, connected = 1, None
    while True:
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.connect(sock_path)
                s.sendall(
                    b"GET /api/v1/events HTTP/1.0\r\nHost: localhost\r\n"
                    b"Accept: text/event-stream\r\n\r\n"
                )
                f = s.makefile("rb")
                while f.readline() not in (b"\r\n", b"\n", b""):
                    pass
                if connected is not True:
                    log(f"{server}: watching events")
                connected, delay = True, 1
                for event_type, data in sse_events(f):
                    kind = event_kind(event_type)
                    session_id = session_id_of(data) if kind else None
                    if session_id:
                        events.put((server, kind, session_id))
        except OSError as e:
            if connected is not False:
                log(f"{server}: no event stream ({e}); retrying")
        else:
            log(f"{server}: event stream ended; reconnecting")
        connected = False
        events.put((server, "disconnect", None))
        time.sleep(delay)
        delay = min(delay * 2, 30)


def main():
    idle = float(os.environ.get("WOLF_IDLE_MINUTES", "30")) * 60
    tracker = Tracker(SERVERS, idle)
    events = queue.Queue()
    for server, path in SERVERS.items():
        threading.Thread(target=watch, args=(server, path, events), daemon=True).start()
    while True:
        try:
            server, kind, session_id = events.get(timeout=30)
        except queue.Empty:
            pass
        else:
            if kind == "disconnect":
                tracker.on_disconnect(server)
            else:
                for other, sid in tracker.on_event(server, kind, session_id, time.monotonic()):
                    stop(other, sid, f"started on {server}")
        for server, sid in tracker.due(time.monotonic()):
            stop(server, sid, f"paused for {idle / 60:g} min")


if __name__ == "__main__":
    main()
