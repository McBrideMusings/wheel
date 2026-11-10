"""A throwaway wheeld with a four-pane space, for running and checking the GPUI client.

The daemon and `wheel` are copied out of the installed Wheel.app and ad-hoc signed, so an
unrelated client install cannot stop them. Nothing here touches the real wheeld.
"""
import contextlib, json, os, pathlib, shutil, socket, subprocess, tempfile, time

DAEMON = '/Applications/Wheel.app/Contents/Helpers/Wheel Server.wheelserver/Contents/MacOS/wheeld'


@contextlib.contextmanager
def fixture():
    """Yield (env, info). `env` carries WHEEL_URL, WHEEL_TOKEN_FILE and WHEEL_CLIENT_SPACE."""
    env = os.environ.copy()
    for key in ['WHEEL_PANE', 'WHEEL_SPACE', 'WHEEL_SPACE_DIR']:
        env.pop(key, None)
    with tempfile.TemporaryDirectory(prefix='wheel-gpui-fixture-') as temp:
        root = pathlib.Path(temp)
        project = root / 'project'
        page = project / 'html'
        page.mkdir(parents=True)
        (page / 'index.html').write_text('''<!doctype html><html><meta charset="utf-8"><title>Fixture artifact</title>
<style>body{background:#e9eee8;color:#1c3027;font:18px/1.6 system-ui;padding:22px}</style>
<h1>HTML artifact</h1><p>Served by the fixture wheeld into the client's WKWebView pane.</p></html>''')
        (project / 'wheel.toml').write_text('[artifacts.fixture-html]\npath="html"\n')
        (root / 'machine.toml').write_text('')
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        url = f'ws://127.0.0.1:{port}/ws'
        token = root / 'support/token'
        shutil.copy2(os.environ.get('WHEEL_FIXTURE_DAEMON', DAEMON), root / 'wheeld')
        shutil.copy2(shutil.which('wheel'), root / 'wheel')
        for executable in ['wheeld', 'wheel']:
            subprocess.run(['codesign', '--force', '--sign', '-', str(root / executable)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

        def cli(*args):
            return subprocess.check_output([str(root / 'wheel'), '--url', url, '--token-file', str(token), *args], env=env, text=True, timeout=40).strip()

        def tree(space):
            return next(s for s in json.loads(cli('status', '--json'))['spaces'] if s['id'] == space)

        log = open(root / 'daemon.log', 'a')
        running = []

        def start():
            daemon = subprocess.Popen([str(root / 'wheeld'), '--listen', f'127.0.0.1:{port}', '--support-dir', str(root / 'support'),
                '--machine-file', str(root / 'machine.toml'), '--log-dir', str(root / 'logs')], env=env, stdout=log, stderr=log)
            running.append(daemon)
            for _ in range(100):
                if daemon.poll() is not None:
                    raise RuntimeError(f'fixture daemon exited: {daemon.returncode}')
                if token.exists():
                    try:
                        with socket.create_connection(('127.0.0.1', port), .1):
                            return daemon
                    except OSError:
                        pass
                time.sleep(.1)
            raise TimeoutError('fixture daemon did not start')

        def stop():
            daemon = running[-1]
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()

        def restart(down_for=0.0):
            """Stop the daemon, wait `down_for` seconds with nothing listening, start it again on the same port and support dir."""
            stop()
            time.sleep(down_for)
            return start().pid

        daemon = start()
        try:
            space = cli('space', 'new', str(project)).splitlines()[-1]
            web = tree(space)['root']['id']
            cli('pane', 'kind', web, 'web', '--artifact', 'fixture-html')
            term_pane = cli('pane', 'split', web, 'right').splitlines()[-1]
            cli('pane', 'kind', term_pane, 'terminal')
            browser = cli('pane', 'split', term_pane, 'down').splitlines()[-1]
            cli('pane', 'kind', browser, 'browser', '--page', 'about:blank')
            term2_pane = cli('pane', 'split', web, 'down').splitlines()[-1]
            cli('pane', 'kind', term2_pane, 'terminal')
            web2 = cli('pane', 'split', term2_pane, 'down').splitlines()[-1]
            cli('pane', 'kind', web2, 'web', '--artifact', 'fixture-html')

            def terminals(node):
                if node.get('node') == 'pane':
                    return [node] if node.get('kind') == 'terminal' else []
                return terminals(node['a']) + terminals(node['b'])
            markers = {}
            for n, node in enumerate(terminals(tree(space)['root'])):
                marker = f'terminal-{42 + n}'
                markers[node['id']] = marker
                cli('term', 'send', node['term'], "stty -echo; printf '\\033[2J\\033[H\\033[32m%s\\033[0m\\r\\n' " + marker + "; while IFS= read -r line; do printf 'PTY:%s\\r\\n' \"$line\"; done", '--enter')
                cli('term', 'read', node['term'], '--until', marker, '--timeout', '10')
            env.update(WHEEL_URL=url, WHEEL_TOKEN_FILE=str(token), WHEEL_CLIENT_SPACE=space, WHEEL_CLIENT_CLI=str(root / 'wheel'), WHEEL_CLIENT_MARKERS=json.dumps(markers))
            info = dict(space=space, daemon_pid=daemon.pid, port=port, panes=dict(web=web, terminal=term_pane, browser=browser, terminal2=term2_pane, web2=web2),
                        saved_tree=lambda: tree(space)['root'], daemon_alive=lambda: running[-1].poll() is None, restart=restart)
            yield env, info
        finally:
            stop()
            log.close()
