"""A toy plugin backend: speaks Wheel's JSON-RPC (Content-Length framing) on stdin and stdout,
or on the unix socket named in WHEEL_PLUGIN_SOCKET when it is set.

Feeds: `now` sends {"time", "tick"} every second; `lines` sends {"lines": []} once, then a
JSON Patch appending "line <n>" every second.

Actions: `echo` returns {"echo": args, "caller": caller}; `fail` answers a JSON-RPC error; `hidden`
is listed here but not by the manifest, so wheeld refuses it before it gets here.
"""
import json
import os
import socket
import sys
import threading
import time


def log(text):
    print(text, file=sys.stderr, flush=True)


path = os.environ.get("WHEEL_PLUGIN_SOCKET")
if path:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    rfile = sock.makefile("rb")
    wfile = sock.makefile("wb")
else:
    rfile = sys.stdin.buffer
    wfile = sys.stdout.buffer

write_lock = threading.Lock()


def send(message):
    body = json.dumps(message).encode()
    with write_lock:
        wfile.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        wfile.flush()


def read():
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
    if length is None:
        return None
    return json.loads(rfile.read(length))


def key(feed, params):
    return feed + json.dumps(params, sort_keys=True)


# One sender thread per subscribed (feed, params); removing the key stops it.
subscribed = {}


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


log("clock backend up")
while True:
    message = read()
    if message is None:
        break
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
        break
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"],
              "error": {"code": -32601, "message": "no such method"}})
