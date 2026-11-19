#!/usr/bin/env python3
"""⌘C, ⌘X and ⌘V in a browser pane's text field, sent the way a hardware key arrives (the bridge's
`app_key` posts it to the app's event queue), in the hidden dev client against a fixture wheeld.
Prints what the field and the pasteboard hold after each key and the bridge's key log. Never
shows a window; the pasteboard is put back as it was."""
import json, os, pathlib, subprocess, sys, time, urllib.request

here = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(here))
from fixture import fixture  # noqa: E402

app = here.parent / 'target/app/Wheel Dev.app/Contents/MacOS'
PORT = int(os.environ.get('PROBE_PORT', '7499'))
MODE = os.environ.get('PROBE_MODE', 'post')
MAKE_KEY = os.environ.get('PROBE_MAKE_KEY', '1') == '1'
PRETEND = os.environ.get('PROBE_PRETEND', '0') == '1'
out = here.parent / 'target/paste-probe'
out.mkdir(parents=True, exist_ok=True)


def main():
    with fixture() as (env, info):
        token = pathlib.Path(env['WHEEL_TOKEN_FILE']).read_text().strip()
        cli = env['WHEEL_CLIENT_CLI']
        browser = info['panes']['browser']

        def post(path, body=''):
            req = urllib.request.Request(f'http://127.0.0.1:{PORT}{path}', data=body.encode(), method='POST', headers={'Authorization': f'Bearer {token}'})
            return json.loads(urllib.request.urlopen(req, timeout=20).read() or b'null')

        def native(op):
            return post('/native', json.dumps(op))

        def js(code):
            r = subprocess.run([cli, '--url', env['WHEEL_URL'], '--token-file', env['WHEEL_TOKEN_FILE'], 'pane', 'eval', browser, code], capture_output=True, text=True, timeout=30)
            return r.stdout.strip() or r.stderr.strip()

        env.update(RUST_BACKTRACE='1', WHEEL_HIDDEN='1', WHEEL_DEBUG_PORT=str(PORT), WHEEL_GPUI_BRIDGE=str(app / 'libWheelGpuiBridge.dylib'))
        with (out / 'client.log').open('w') as log:
            client = subprocess.Popen([str(app / 'wheel-gpui'), '--url', env['WHEEL_URL'], '--token-file', env['WHEEL_TOKEN_FILE'], '--space', info['space']], env=env, stdout=log, stderr=log)
        report = {}
        try:
            deadline = time.time() + 60
            while True:
                try:
                    urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{PORT}/state', headers={'Authorization': f'Bearer {token}'}), timeout=2)
                    break
                except Exception:
                    if time.time() > deadline or client.poll() is not None:
                        raise RuntimeError(f'the client never answered; see {out / "client.log"}')
                    time.sleep(0.3)
            # The browser's page has to have loaded before an eval reaches it.
            for _ in range(60):
                if js('1+1') == '2':
                    break
                time.sleep(0.5)
            native({'op': 'pasteboard', 'save': True})
            try:
                js("(() => { const t = document.createElement('input'); t.id = 'f'; t.value = 'hello world'; document.body.append(t); window.ev = [];"
                   " for (const k of ['keydown','copy','cut','paste']) t.addEventListener(k, e => ev.push(k + (e.key ? ':' + e.key : ''))); t.focus(); return 1; })()")
                native({'op': 'focus', 'pane': browser})
                time.sleep(0.3)
                steps = []
                for key, code, before in [('c', 8, "f.setSelectionRange(0, 5)"), ('x', 7, "f.setSelectionRange(6, 11)"), ('v', 9, "f.setSelectionRange(6, 6)")]:
                    js(f"(() => {{ const f = document.getElementById('f'); f.focus(); {before}; return 1; }})()")
                    if key == 'c':
                        native({'op': 'pasteboard', 'set': 'untouched'})
                    sent = native({'op': 'app_key', 'key': key, 'code': code, 'mode': MODE, 'make_key': MAKE_KEY, 'pretend_key': PRETEND})
                    time.sleep(1.0)
                    steps.append({'key': f'cmd-{key}', 'sent': sent, 'field': js("document.getElementById('f').value"),
                                  'pasteboard': native({'op': 'pasteboard'}).get('text'), 'events': js("ev.join(' ')")})
                report['steps'] = steps
            finally:
                native({'op': 'pasteboard', 'restore': True})
            report['bridge_log'] = [l for l in native({'op': 'status'}).get('logs', []) if 'WHEELDIAG' in l]
            ok = [s['pasteboard'] for s in report['steps']] == ['hello', 'world', 'world'] and report['steps'][2]['field'] == '"hello world"'
            report['pass'] = ok
        finally:
            client.terminate()
            client.wait(timeout=10)
        print(json.dumps(report, indent=2, ensure_ascii=False))
        sys.exit(0 if report.get('pass') else 1)


main()
