"""The gallery's backend: serves the video stream `demo` (ADR-0053) that the gallery's VideoStream
shows, and the feed `input`, the last input event a client sent on it.

It speaks Wheel's JSON-RPC (Content-Length framing) on stdin and stdout, and listens on
`$WHEEL_PLUGIN_STREAM_DIR/demo.sock`. To each client that connects it sends the parameter sets of
`demo.h264` (a 2 s, 320x180, 30 fps H.264 clip), then its access units in a loop at 30 fps, each
followed by 1/30 s of a 440 Hz tone (48 kHz, 2 channels). Each input frame the client sends is
logged to stderr and published on `input`.

Frames on the socket, both ways: a type byte, a 4-byte big-endian length, the payload.
  1 parameter sets: codec byte (1 H.264, 2 HEVC), then the Annex-B parameter set NAL units
  2 video: 8-byte big-endian time in microseconds, then one Annex-B access unit
  3 audio: 4-byte big-endian sample rate, channel count byte, interleaved s16 little-endian PCM
  16 key, 17 pointer, 18 controller (client to backend): a UTF-8 JSON object
"""
import json
import math
import os
import socket
import struct
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FPS = 30
RATE = 48000
INPUT_KINDS = {16: "key", 17: "pointer", 18: "controller"}


def log(text):
    print(text, file=sys.stderr, flush=True)


def nal_units(data):
    """The NAL units of an Annex-B byte string, start codes removed."""
    units, i, n = [], 0, len(data)
    starts = []
    while i < n - 2:
        if data[i] == 0 and data[i + 1] == 0 and data[i + 2] == 1:
            starts.append(i + 3)
            i += 3
        else:
            i += 1
    for k, start in enumerate(starts):
        end = starts[k + 1] - 3 if k + 1 < len(starts) else n
        unit = data[start:end]
        while unit.endswith(b"\x00") and k + 1 < len(starts):
            unit = unit[:-1]
        units.append(unit)
    return units


def annexb(units):
    return b"".join(b"\x00\x00\x00\x01" + u for u in units)


def clip():
    """The clip's parameter sets and its access units: each slice NAL closes one frame, with the
    non-slice units before it."""
    with open(os.path.join(HERE, "demo.h264"), "rb") as f:
        units = nal_units(f.read())
    params = [u for u in units if u[0] & 0x1F in (7, 8)]
    frames, pending = [], []
    for u in units:
        kind = u[0] & 0x1F
        if kind in (7, 8):
            continue
        pending.append(u)
        if kind in (1, 5):
            frames.append(annexb(pending))
            pending = []
    return annexb(params[:2]), frames


def frame(kind, payload):
    return struct.pack(">BI", kind, len(payload)) + payload


def tone(start):
    """1/30 s of a 440 Hz tone from sample `start`, stereo s16 little-endian."""
    count = RATE // FPS
    out = bytearray()
    for i in range(start, start + count):
        v = int(3000 * math.sin(2 * math.pi * 440 * i / RATE))
        out += struct.pack("<hh", v, v)
    return bytes(out)


class Input:
    """The `input` feed: the last event, its count, and who reads it."""

    def __init__(self, send):
        self.send, self.lock = send, threading.Lock()
        self.value = {"last": None, "count": 0}
        self.readers = []

    def subscribe(self, params):
        with self.lock:
            self.readers.append(params)
            value = dict(self.value)
        self.send({"jsonrpc": "2.0", "method": "feed/update", "params": {"feed": "input", "params": params, "value": value}})

    def unsubscribe(self, params):
        with self.lock:
            if params in self.readers:
                self.readers.remove(params)

    def event(self, kind, body):
        with self.lock:
            self.value = {"last": "%s %s" % (kind, body), "count": self.value["count"] + 1}
            value, readers = dict(self.value), list(self.readers)
        for params in readers:
            self.send({"jsonrpc": "2.0", "method": "feed/update", "params": {"feed": "input", "params": params, "value": value}})


def stream_to(conn, params, frames, feed):
    def read_input():
        buf = b""
        while True:
            try:
                chunk = conn.recv(65536)
            except OSError:
                return
            if not chunk:
                return
            buf += chunk
            while len(buf) >= 5:
                kind, length = struct.unpack(">BI", buf[:5])
                if len(buf) < 5 + length:
                    break
                body, buf = buf[5:5 + length].decode(errors="replace"), buf[5 + length:]
                name = INPUT_KINDS.get(kind, "type %d" % kind)
                log("input %s %s" % (name, body))
                feed.event(name, body)

    threading.Thread(target=read_input, daemon=True).start()
    try:
        conn.sendall(frame(1, b"\x01" + params))
        started, n = time.monotonic(), 0
        while True:
            at = n / FPS
            wait = started + at - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            conn.sendall(frame(2, struct.pack(">Q", int(at * 1_000_000)) + frames[n % len(frames)]))
            conn.sendall(frame(3, struct.pack(">IB", RATE, 2) + tone(n * (RATE // FPS))))
            n += 1
    except OSError as e:
        log("stream client gone: %s" % e)
    finally:
        conn.close()


def listen(feed):
    folder = os.environ.get("WHEEL_PLUGIN_STREAM_DIR")
    if not folder:
        log("no WHEEL_PLUGIN_STREAM_DIR: the demo stream is not served")
        return
    path = os.path.join(folder, "demo.sock")
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen()
    params, frames = clip()
    log("serving demo on %s: %d frames" % (path, len(frames)))
    while True:
        conn, _ = server.accept()
        log("stream client connected")
        threading.Thread(target=stream_to, args=(conn, params, frames, feed), daemon=True).start()


def read_message(rfile):
    length = None
    while True:
        line = rfile.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    return None if length is None else rfile.read(length)


def main():
    rfile, wfile, lock = sys.stdin.buffer, sys.stdout.buffer, threading.Lock()

    def send(message):
        body = json.dumps(message).encode()
        with lock:
            wfile.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
            wfile.flush()

    feed = Input(send)
    threading.Thread(target=listen, args=(feed,), daemon=True).start()
    while True:
        body = read_message(rfile)
        if body is None:
            return
        message = json.loads(body)
        method, params = message.get("method"), message.get("params") or {}
        if method == "initialize":
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"feeds": ["input"], "actions": [], "streams": ["demo"]}})
        elif method == "feed/subscribe" and params.get("feed") == "input":
            feed.subscribe(params.get("params", {}))
        elif method == "feed/unsubscribe" and params.get("feed") == "input":
            feed.unsubscribe(params.get("params", {}))
        elif method == "shutdown":
            return
        elif "id" in message:
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "no such method"}})


if __name__ == "__main__":
    main()
