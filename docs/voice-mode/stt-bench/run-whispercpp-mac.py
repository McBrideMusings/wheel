"""Mac whisper.cpp via Homebrew whisper-server (model held loaded): same JSON shape as the Swift bench. Usage: run-whispercpp-mac.py <model.bin> <out.json>"""
import json, os, subprocess, sys, time, urllib.request, uuid

B = os.path.dirname(os.path.abspath(__file__))
PORT = 18181
model, out = sys.argv[1], sys.argv[2]


def post(path):
    boundary = uuid.uuid4().hex
    data = open(path, "rb").read()
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").encode() + data + \
        (f"\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n"
         f"--{boundary}\r\nContent-Disposition: form-data; name=\"temperature\"\r\n\r\n0\r\n--{boundary}--\r\n").encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/inference", data=body, headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
    return json.load(urllib.request.urlopen(req, timeout=120))["text"]


def rss_mb(pid):
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)]).strip()) / 1024


t0 = time.time()
srv = subprocess.Popen(["whisper-server", "-m", model, "--port", str(PORT), "-l", "en", "-nt", "-fa", "-t", "8"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    first = f"{B}/fixture/clean/u01.wav"
    while True:
        try:
            post(first); break
        except Exception:
            if srv.poll() is not None: sys.exit("whisper-server exited")
            time.sleep(0.2)
    load_s = time.time() - t0
    peak, clips = rss_mb(srv.pid), []
    for st in ("clean", "noisy"):
        for f in sorted(os.listdir(f"{B}/fixture/{st}")):
            p = f"{B}/fixture/{st}/{f}"
            t = time.time(); text = post(p); dt = time.time() - t
            peak = max(peak, rss_mb(srv.pid))
            dur = (os.path.getsize(p) - 44) / 32000
            clips.append({"set": st, "id": f[:-4], "audio_s": dur, "latency_s": dt, "text": text})
            print(f"{st}/{f} {dt:.3f}s {text.strip()}")
    json.dump({"platform": "Mac", "engine": "whisper.cpp/" + os.path.basename(model), "load_s": load_s, "peak_mb": peak, "clips": clips}, open(out, "w"), indent=1)
    print(f"wrote {out} load {load_s:.1f}s peak {peak:.0f} MB (RSS, first post included in load)")
finally:
    srv.terminate(); srv.wait()
