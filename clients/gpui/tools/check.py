#!/usr/bin/env python3
"""Hidden regression check: the client against a fixture wheeld. Never shows a window."""
import hashlib, json, pathlib, plistlib, subprocess, sys, time

here = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(here))
from fixture import fixture  # noqa: E402

out = here.parent / 'target/check'
out.mkdir(parents=True, exist_ok=True)
app = here.parent / 'target/app/Wheel GPUI.app/Contents/MacOS'
host = app / 'wheel-gpui-check'
contents = app.parent


def bundled_server():
    """The server the app carries, as the acceptance names it: the .wheelserver bundle, signed, its
    script and its plist whose SourceHash is the bundled wheeld's own `--source-hash`."""
    read = lambda path: plistlib.loads(path.read_bytes())  # noqa: E731
    info = read(contents / 'Resources/WheelDaemon.plist')
    server = contents / 'Helpers' / f"{info['ServerName']}.wheelserver"
    wheeld = server / 'Contents/MacOS/wheeld'
    source = subprocess.run([str(wheeld), '--source-hash'], check=True, capture_output=True, text=True).stdout.strip()
    signed = subprocess.run(['codesign', '--verify', '--strict', str(server)], capture_output=True, text=True)
    bundle_id = read(server / 'Contents/Info.plist')['CFBundleIdentifier']
    assert source == info['SourceHash'], f'WheelDaemon.plist says {info["SourceHash"]}, the bundled wheeld says {source}'
    assert signed.returncode == 0, f'codesign --verify: {signed.stderr}'
    assert (contents / 'Resources/wheel-server.sh').exists() and (contents / 'Helpers/wheel').exists() and (contents / 'Helpers/wheel-runner').exists()
    assert bundle_id == info['Label'], f'the server bundle is {bundle_id}, the agent label {info["Label"]}'
    return dict(server=str(server), label=info['Label'], instance=info['Instance'], listen=info['Listen'], source=source, signed=True, icon=(server / 'Contents/Resources/ServerIcon.icns').exists())


def app_bundle():
    """The app bundle as `tools/build.py release` makes it: its plist, its icon and its signature."""
    plist = plistlib.loads((contents / 'Info.plist').read_bytes())
    verified = subprocess.run(['codesign', '--verify', '--strict', str(contents.parent)], capture_output=True, text=True)
    signer = subprocess.run(['codesign', '--display', '--verbose=2', str(contents.parent)], capture_output=True, text=True).stderr
    assert plist['CFBundleIdentifier'] == 'com.piercemakes.wheel.gpui' and plist['CFBundleName'] == 'Wheel GPUI', plist
    assert (contents / 'Resources' / f"{plist['CFBundleIconFile']}.icns").exists(), 'no app icon'
    assert verified.returncode == 0, f'codesign --verify of the app: {verified.stderr}'
    return dict(bundle_id=plist['CFBundleIdentifier'], name=plist['CFBundleName'], category=plist['LSApplicationCategoryType'],
                icon=plist['CFBundleIconFile'], signed=True, authority=next((l.split('=', 1)[1] for l in signer.splitlines() if l.startswith('Authority=')), 'ad hoc'))


with fixture() as (env, info):
    env.update(WHEEL_CLIENT_RESULTS=str(out), WHEEL_GPUI_BRIDGE=str(app / 'libWheelGpuiBridge.dylib'),
               # What Restart Server runs for a wheeld the client did not install: the check reads the two files.
               WHEEL_SERVER_STOP=f'touch {out / "server.stop"}', WHEEL_SERVER_START=f'touch {out / "server.start"}')
    for name in ['server.stop', 'server.start']:
        (out / name).unlink(missing_ok=True)
    (out / 'report.json').unlink(missing_ok=True)
    digest = hashlib.sha256(host.read_bytes()).hexdigest()
    for name in ['restart.request', 'restart.done']:
        (out / name).unlink(missing_ok=True)
    with (out / 'host.log').open('w') as log:
        process = subprocess.Popen([str(host)], env=env, stdout=log, stderr=log)
        try:
            deadline = time.time() + 420
            while process.poll() is None:
                if time.time() > deadline:
                    process.kill()
                    raise TimeoutError('client check timed out')
                request = out / 'restart.request'
                if request.exists():
                    down_for = float(request.read_text())
                    request.unlink()
                    info['restarted_daemon_pid'] = info['restart'](down_for)
                    (out / 'restart.done').write_text(str(info['restarted_daemon_pid']))
                time.sleep(0.05)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, str(host))
    report = json.loads((out / 'report.json').read_text())
    assert hashlib.sha256(host.read_bytes()).hexdigest() == digest
    bundled = bundled_server()
    app_info = app_bundle()
    report.update(daemon_pid=info['daemon_pid'], panes=info['panes'], saved_tree_after=info['saved_tree'](),
                  daemon_alive_after_client_exit=info['daemon_alive'](), host_sha256=digest, bundled_server=bundled, app_bundle=app_info)
    (out / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False))
    print(json.dumps(dict(host_pid=report['host_pid'], daemon_pid=report['daemon_pid'], restarted_daemon_pid=info['restarted_daemon_pid'], restart=report['restart']['saw_down'], server_updated=report['server_updated'], server_lifecycle=report['server_lifecycle'], bundled_server=report['bundled_server'], app_bundle=report['app_bundle'], layouts=report['layouts_received'],
                          ratio_saved=report['drag']['ratio_saved_by_wheeld'], markdown=report['markdown'], scroll=report['scroll'], footer=report['footer'], status=report['status'], sidebar=report['sidebar'], title_row=report['title_row'], web=report['web'], browser=report['browser'], commands=report['commands']['panes'], settings={k: report['settings'][k] for k in ['typed_saved', 'cli_write_seen', 'cleared_to', 'refusal']} | dict(chat=report['settings']['chat']['written'], project=report['settings']['project']['value'], builtins=len(report['settings']['builtins']), devices=report['settings']['devices']), pages={k: (v['state'], v['components'], v['png_bytes']) for k, v in report['pages']['pages'].items()}), indent=2))
