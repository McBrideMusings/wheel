"""A throwaway wheeld with a five-pane space, for running and checking the GPUI client.

The daemon and `wheel` are copied out of the installed Wheel.app and ad-hoc signed, so an
unrelated client install cannot stop them. Nothing here touches the real wheeld.
"""
import contextlib, json, os, pathlib, shutil, socket, subprocess, tempfile, time

DAEMON = '/Applications/Wheel.app/Contents/Helpers/Wheel Server.wheelserver/Contents/MacOS/wheeld'
# The agent pane runs the daemon's scripted stand-in for Claude Code (`daemon/src/bin/wheel-fake-agent.rs`):
# the same stream-json wire, no model calls. Built by `cargo build -p wheeld --bin wheel-fake-agent`.
FAKE_AGENT = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', pathlib.Path(__file__).resolve().parents[3] / 'target')) / 'debug/wheel-fake-agent'


def html_page(title):
    """The HTML artifact's page under `title`; the check rewrites it to see the pane reload."""
    return f'''<!doctype html><html><meta charset="utf-8"><title>{title}</title>
<style>body{{background:#e9eee8;color:#1c3027;font:18px/1.6 system-ui;padding:22px}}</style>
<h1>{title}</h1><p>Served by the fixture wheeld into the client's WKWebView pane.</p>
<script src=".wheel/client.js"></script><script>wheel.set('seen', {{ok: true, title: document.title}})</script></html>'''


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
        # The page calls the daemon through `.wheel/client.js`: `wheel artifact state <pane> seen` reads what it stored.
        (page / 'index.html').write_text(html_page('Fixture artifact'))
        gallery = pathlib.Path(__file__).resolve().parents[3] / 'tools/gallery/server.py'
        (project / 'wheel.toml').write_text(f'[artifacts.fixture-html]\npath="html"\npane=true\n\n[artifacts.fixture-page]\nmcp="python3 {gallery} v17 filters-form"\npane=true\n')
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
            cli('pane', 'kind', web2, 'web', '--artifact', 'fixture-page')
            fake_agent = pathlib.Path(os.environ.get('WHEEL_FIXTURE_FAKE_AGENT', FAKE_AGENT))
            if not fake_agent.exists():
                raise FileNotFoundError(f'{fake_agent} is missing: run `cargo build -p wheeld --bin wheel-fake-agent` in the repo')
            cli('config', 'set', 'agent.claude.command', str(fake_agent), '--machine')
            agent_pane = cli('pane', 'split', browser, 'down').splitlines()[-1]
            started = json.loads(cli('pane', 'start', agent_pane, 'agent', '--agent', 'claude'))

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
            second_dir = root / 'project2'
            second_dir.mkdir()
            second_session = cli('session', 'new', 'Second').splitlines()[-1]
            second_space = cli('space', 'new', str(second_dir), '--name', 'other', '--session', second_session).splitlines()[-1]
            env.update(WHEEL_CLIENT_SECOND_SESSION=second_session, WHEEL_CLIENT_SECOND_SPACE=second_space)
            env.update(WHEEL_URL=url, WHEEL_TOKEN_FILE=str(token), WHEEL_CLIENT_SPACE=space, WHEEL_CLIENT_CLI=str(root / 'wheel'), WHEEL_CLIENT_MARKERS=json.dumps(markers),
                       WHEEL_CLIENT_AGENT_PANE=agent_pane, WHEEL_CLIENT_AGENT=started['agent'], WHEEL_CLIENT_HTML=str(page))
            info = dict(space=space, daemon_pid=daemon.pid, port=port, panes=dict(web=web, terminal=term_pane, browser=browser, terminal2=term2_pane, web2=web2, agent=agent_pane),
                        saved_tree=lambda: tree(space)['root'], daemon_alive=lambda: running[-1].poll() is None, restart=restart)
            yield env, info
        finally:
            stop()
            log.close()
