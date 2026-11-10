#!/usr/bin/env python3
"""Hidden regression check: the client against a fixture wheeld. Never shows a window."""
import hashlib, json, pathlib, subprocess, sys

here = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(here))
from fixture import fixture  # noqa: E402

out = here.parent / 'target/check'
out.mkdir(parents=True, exist_ok=True)
app = here.parent / 'target/app/Wheel GPUI.app/Contents/MacOS'
host = app / 'wheel-gpui-check'

with fixture() as (env, info):
    env.update(WHEEL_CLIENT_RESULTS=str(out), WHEEL_GPUI_BRIDGE=str(app / 'libWheelGpuiBridge.dylib'))
    (out / 'report.json').unlink(missing_ok=True)
    digest = hashlib.sha256(host.read_bytes()).hexdigest()
    with (out / 'host.log').open('w') as log:
        subprocess.run([str(host)], env=env, stdout=log, stderr=log, check=True, timeout=240)
    report = json.loads((out / 'report.json').read_text())
    assert hashlib.sha256(host.read_bytes()).hexdigest() == digest
    report.update(daemon_pid=info['daemon_pid'], panes=info['panes'], saved_tree_after=info['saved_tree'](),
                  daemon_alive_after_client_exit=info['daemon_alive'](), host_sha256=digest)
    (out / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False))
    print(json.dumps(dict(host_pid=report['host_pid'], daemon_pid=report['daemon_pid'], layouts=report['layouts_received'],
                          ratio_saved=report['drag']['ratio_saved_by_wheeld']), indent=2))
