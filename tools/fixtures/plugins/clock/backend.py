"""A toy plugin backend: speaks Wheel's JSON-RPC (Content-Length framing) on stdin and stdout,
or on the unix socket named in WHEEL_PLUGIN_SOCKET when it is set.

Run by hand as an [[external_backend]] it listens instead and serves connections side by side
until killed, each in its own thread:

    python3 backend.py --listen unix:/path/to.sock
    python3 backend.py --listen ws:<port> --token <token>   # one JSON-RPC message per text message

A WebSocket client whose `Authorization` header is not `Bearer <token>` is answered 401.

Feeds: `now` sends {"time", "tick"} every second; `lines` sends {"lines": []} once, then a
JSON Patch appending "line <n>" every second.

Actions: `echo` returns {"echo": args, "caller": caller}; `fail` answers a JSON-RPC error; `hidden`
is listed here but not by the manifest, so wheeld refuses it before it gets here.
"""
import base64
import hashlib
import json
import os
import shutil
import socket
import struct
import sys
import threading
import time


def log(text):
    print(text, file=sys.stderr, flush=True)


class Framed:
    """Content-Length framed messages over a pair of binary files."""

    def __init__(self, rfile, wfile):
        self.rfile, self.wfile = rfile, wfile

    def send(self, body):
        self.wfile.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.wfile.flush()

    def read(self):
        length = None
        while True:
            line = self.rfile.readline()
            if not line:
                return None
            line = line.strip()
            if not line:
                break
            name, _, value = line.decode().partition(":")
            if name.lower() == "content-length":
                length = int(value)
        if length is None:
            return None
        return self.rfile.read(length)


class WebSocket:
    """The server side of one WebSocket, after the handshake: text messages in and out."""

    def __init__(self, sock, rfile):
        self.sock, self.rfile = sock, rfile

    def send(self, body):
        head = bytearray([0x81])
        n = len(body)
        if n < 126:
            head.append(n)
        elif n < 65536:
            head.append(126)
            head += struct.pack(">H", n)
        else:
            head.append(127)
            head += struct.pack(">Q", n)
        self.sock.sendall(bytes(head) + body)

    def read(self):
        message = b""
        while True:
            head = self.rfile.read(2)
            if len(head) < 2:
                return None
            fin, opcode = head[0] & 0x80, head[0] & 0x0F
            n = head[1] & 0x7F
            if n == 126:
                n = struct.unpack(">H", self.rfile.read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self.rfile.read(8))[0]
            mask = self.rfile.read(4) if head[1] & 0x80 else b"\0\0\0\0"
            data = bytes(b ^ mask[i % 4] for i, b in enumerate(self.rfile.read(n)))
            if opcode == 0x8:
                return None
            if opcode == 0x9:
                self.sock.sendall(bytes([0x8A, len(data)]) + data)
                continue
            if opcode in (0x0, 0x1, 0x2):
                message += data
                if fin:
                    return message


def ws_handshake(sock, rfile, token):
    """Read the HTTP upgrade request and answer it; None when it is refused."""
    headers = {}
    request = rfile.readline()
    while True:
        line = rfile.readline().decode().strip()
        if not line:
            break
        name, _, value = line.partition(":")
        headers[name.strip().lower()] = value.strip()
    if headers.get("authorization") != "Bearer " + token:
        log("refused a WebSocket with authorization %r" % headers.get("authorization"))
        sock.sendall(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        return None
    accept = base64.b64encode(hashlib.sha1(
        (headers.get("sec-websocket-key", "") + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
    sock.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                 b"Sec-WebSocket-Accept: " + accept + b"\r\n\r\n")
    log("accepted a WebSocket for " + request.decode().split(" ")[1])
    return WebSocket(sock, rfile)


def key(feed, params):
    return feed + json.dumps(params, sort_keys=True)


def serve(channel):
    """Answer one connection until it ends or says shutdown. Returns True on shutdown."""
    write_lock = threading.Lock()
    # One sender thread per subscribed (feed, params); removing the key stops it.
    subscribed = {}

    def send(message):
        with write_lock:
            try:
                channel.send(json.dumps(message).encode())
            except OSError:
                subscribed.clear()

    def run_now(feed, params, token):
        tick = 0
        while subscribed.get(key(feed, params)) is token:
            tick += 1
            send({"jsonrpc": "2.0", "method": "feed/update", "params": {
                "feed": feed, "params": params,
                "value": {"time": time.strftime("%H:%M:%S", time.gmtime()), "tick": tick}}})
            time.sleep(1)

    def run_lines(feed, params, token):
        send({"jsonrpc": "2.0", "method": "feed/update", "params": {
            "feed": feed, "params": params, "value": {"lines": []}}})
        n = 0
        while subscribed.get(key(feed, params)) is token:
            time.sleep(1)
            if subscribed.get(key(feed, params)) is not token:
                break
            n += 1
            send({"jsonrpc": "2.0", "method": "feed/update", "params": {
                "feed": feed, "params": params,
                "patch": [{"op": "add", "path": "/lines/-", "value": "line %d" % n}]}})

    try:
        while True:
            try:
                body = channel.read()
            except OSError:
                body = None
            if body is None:
                return False
            message = json.loads(body)
            method = message.get("method")
            params = message.get("params") or {}
            if method == "initialize":
                send({"jsonrpc": "2.0", "id": message["id"],
                      "result": {"feeds": ["now", "lines"], "actions": ["echo", "fail", "hidden"]}})
                send({"jsonrpc": "2.0", "method": "log",
                      "params": {"level": "info", "message": "initialized for " + params.get("plugin_id", "?")}})
            elif method == "feed/subscribe":
                feed, feed_params = params["feed"], params.get("params", {})
                token = object()
                subscribed[key(feed, feed_params)] = token
                target = {"now": run_now, "lines": run_lines}.get(feed)
                if target:
                    threading.Thread(target=target, args=(feed, feed_params, token), daemon=True).start()
            elif method == "feed/unsubscribe":
                feed, feed_params = params["feed"], params.get("params", {})
                subscribed.pop(key(feed, feed_params), None)
                log("unsubscribed " + feed)
            elif method == "action/invoke":
                if params.get("action") == "echo":
                    send({"jsonrpc": "2.0", "id": message["id"],
                          "result": {"echo": params.get("args"), "caller": params.get("caller")}})
                elif params.get("action") == "fail":
                    send({"jsonrpc": "2.0", "id": message["id"],
                          "error": {"code": -32000, "message": "the clock refuses"}})
                else:
                    send({"jsonrpc": "2.0", "id": message["id"], "result": "hidden ran"})
            elif method == "shutdown":
                return True
            elif "id" in message:
                send({"jsonrpc": "2.0", "id": message["id"],
                      "error": {"code": -32601, "message": "no such method"}})
    finally:
        subscribed.clear()


def listen(address, token):
    """Serve every connection on `unix:<path>` or `ws:<port>`, each in its own thread, until killed."""
    kind, _, where = address.partition(":")
    if kind == "unix":
        if os.path.exists(where):
            os.remove(where)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(where)
    else:
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", int(where)))
    server.listen(4)
    log("clock backend listening on " + address)

    def one(sock):
        rfile, wfile = sock.makefile("rb"), sock.makefile("wb")
        channel = Framed(rfile, wfile) if kind == "unix" else ws_handshake(sock, rfile, token)
        if channel is not None:
            log("connected")
            serve(channel)
            log("disconnected")
        sock.close()

    while True:
        sock, _ = server.accept()
        threading.Thread(target=one, args=(sock,), daemon=True).start()


log("clock backend up")
log("env WHEEL_URL=%s WHEEL_TOKEN_FILE=%s wheel=%s" % (
    os.environ.get("WHEEL_URL"), os.environ.get("WHEEL_TOKEN_FILE"), shutil.which("wheel")))
args = sys.argv[1:]
if "--listen" in args:
    listen(args[args.index("--listen") + 1], args[args.index("--token") + 1] if "--token" in args else "")
path = os.environ.get("WHEEL_PLUGIN_SOCKET")
if path:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    serve(Framed(sock.makefile("rb"), sock.makefile("wb")))
else:
    serve(Framed(sys.stdin.buffer, sys.stdout.buffer))
