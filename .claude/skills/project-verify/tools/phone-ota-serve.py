#!/usr/bin/env python3
"""Serves one folder on 127.0.0.1 until a client has downloaded Wheel.ipa whole, then exits 0.

GET /status answers what iOS has fetched so far, and POST /log prints what the install page
reports (its taps, its errors), so the page can show the person why an install did not start.

Usage: phone-ota-serve.py <dir> <port> <timeout-seconds>   (exits 3 on timeout)
       phone-ota-serve.py --self-check
"""
import http.server, json, os, socketserver, sys, threading, time, urllib.request

PACKAGE = "/Wheel.ipa"
MANIFEST = "/manifest.plist"


def serve(folder, port, timeout):
    done = threading.Event()
    seen = {"manifest": [], "package": []}

    def note(line):
        print(f"{time.strftime('%H:%M:%S')} {line}", flush=True)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=folder, **k)

        def log_message(self, fmt, *args):
            sys.stderr.write(f"{self.log_date_time_string()} {fmt % args} ua={self.headers.get('User-Agent', '-')}\n")

        def reply(self, code, body=b"", kind="application/json"):
            self.send_response(code)
            self.send_header("Content-Type", kind)
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path.split("?", 1)[0] != "/log":
                return self.reply(404)
            size = min(int(self.headers.get("Content-Length") or 0), 4096)
            text = self.rfile.read(size).decode("utf-8", "replace").replace("\n", " ")
            note(f"page: {text}")
            self.reply(204)

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path == "/status":
                return self.reply(200, json.dumps(seen).encode())
            agent = self.headers.get("User-Agent", "-")
            if path == MANIFEST:
                seen["manifest"].append(time.strftime("%H:%M:%S"))
                note(f"iOS fetched the manifest (ua={agent})")
            if path == PACKAGE:
                note(f"iOS started the package download (ua={agent})")
            super().do_GET()
            # do_GET returns after the body is written, so this marks a finished download.
            if path == PACKAGE and not self.wfile.closed:
                self.wfile.flush()
                seen["package"].append(time.strftime("%H:%M:%S"))
                note(f"served {PACKAGE}")
                done.set()

    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.ThreadingTCPServer(("127.0.0.1", port), Handler) as httpd:
        threading.Thread(target=httpd.serve_forever, daemon=True).start()
        finished = done.wait(timeout)
        # Give the page's next status poll a moment to see the finished download.
        time.sleep(2 if finished else 0)
        httpd.shutdown()
    return 0 if finished else 3


def self_check():
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        with open(os.path.join(d, "Wheel.ipa"), "wb") as f:
            f.write(b"x" * 100_000)
        with open(os.path.join(d, "manifest.plist"), "w") as f:
            f.write("m")
        result = {}
        t = threading.Thread(target=lambda: result.setdefault("code", serve(d, 18765, 10)))
        t.start()
        time.sleep(0.3)
        base = "http://127.0.0.1:18765"
        assert urllib.request.urlopen(base + "/manifest.plist").read() == b"m"
        req = urllib.request.Request(base + "/log", data=b"tap", method="POST")
        assert urllib.request.urlopen(req).status == 204
        status = json.loads(urllib.request.urlopen(base + "/status").read())
        assert len(status["manifest"]) == 1 and status["package"] == [], status
        assert t.is_alive(), "the manifest alone must not end the server"
        assert len(urllib.request.urlopen(base + "/Wheel.ipa?b=1").read()) == 100_000
        t.join(5)
        assert result.get("code") == 0, result
    assert serve(tempfile.gettempdir(), 18766, 0.2) == 3, "a timeout exits 3"
    print("self-check ok")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-check"]:
        sys.exit(self_check())
    folder, port, timeout = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
    sys.exit(serve(folder, port, timeout))
