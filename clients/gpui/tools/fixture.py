"""A throwaway wheeld with a six-pane space, for running and checking the GPUI client.

Every program it runs is the repository's own debug build, from the same proto crate the client is
built with (an installed Wheel.app can speak an older protocol, which the client refuses). wheeld and
`wheel` are copied out and ad-hoc signed, so a rebuild cannot stop them. Nothing here touches the
real wheeld.
"""
import contextlib, json, os, pathlib, shlex, shutil, socket, subprocess, tempfile, time

REPO = pathlib.Path(__file__).resolve().parents[3]
# The repository's cargo target directory: the client's own CARGO_TARGET_DIR is another workspace's.
TARGET = pathlib.Path(os.environ.get('WHEEL_FIXTURE_TARGET', REPO / 'target'))
BUILD = 'cargo build -p wheeld -p wheel-cli -p wheelhouse -p wheel-runner'
# The plugin pages the check opens in the GPUI client, served as the machine artifacts they are on a
# real machine: wheelhouse for Beads, Issues and Repo, wheel-runner for Runner, and the component
# gallery whole. They read the space they are opened in, which is this repository.
PAGES = ['beads', 'issues', 'repo', 'runner', 'gallery']


def built(name):
    """The debug build of `name` under `TARGET`; the agent pane's `wheel-fake-agent` is the
    daemon's scripted stand-in for Claude Code (the same stream-json wire, no model calls)."""
    path = TARGET / 'debug' / name
    if not path.exists():
        raise FileNotFoundError(f'{path} is missing: run `{BUILD}` in the repo')
    return path


def machine_artifacts(gallery):
    """The machine file's artifact entries for `PAGES`, each a shared MCP server pane."""
    wheelhouse = f'{shlex.quote(str(built("wheelhouse")))} serve'
    serve = dict(beads=wheelhouse, issues=wheelhouse, repo=wheelhouse, runner=f'{shlex.quote(str(built("wheel-runner")))} serve',
                 gallery=f'python3 {shlex.quote(str(gallery))} v17')
    # Remote access on an ephemeral loopback port: the Devices page and the pairing window get a code to show.
    return '[remote]\nlisten = "127.0.0.1:0"\n\n' + TITLE_MACHINE + ''.join(f'[artifacts.{name}]\nmcp = {json.dumps(serve[name])}\nshared = true\npane = true\ntitle = {json.dumps(name.title())}\n\n' for name in PAGES)


# The machine's `[[title.button]]` entries (ADR-0037): `build` with a state command that recolours
# and badges it every second, `vpn` with a command, and the built-in pin hidden.
TITLE_MACHINE = '''[[title.button]]
id = "build"
icon = "hammer"
tip = "Build"
command = "echo machine-build > build.txt"
state = "echo '{\\"icon\\":\\"hammer.fill\\",\\"tint\\":\\"green\\",\\"badge\\":\\"2\\"}'"
every = 1

[[title.button]]
id = "vpn"
icon = "lock"
tip = "VPN"
command = "echo machine-pressed > pressed.txt"

[[title.button]]
id = "pin"
hidden = true

'''
# The project's entries: `vpn` replaces the machine's in place, on the right with its own command;
# `deploy` has no command, so it is an indicator.
TITLE_PROJECT = '''
[[title.button]]
id = "vpn"
icon = "lock.open"
side = "right"
tip = "Project VPN"
command = "echo project-pressed > pressed.txt"

[[title.button]]
id = "deploy"
icon = "shippingbox"
'''


def usage_readings():
    """What `wheeld --usage-fixture` reads: two Claude logins (one signed out) and a Codex Pro Lite login."""
    now = int(time.time())
    iso = lambda t: time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(t))  # noqa: E731
    return json.dumps({'accounts': [
        {'name': '.claude', 'five_hour': {'utilization': 3, 'resets_at': iso(now + 7200)}, 'seven_day': {'utilization': 86, 'resets_at': iso(now + 3 * 86400)}},
        {'name': '.claude-work', 'not_logged_in': 'signed out: open Claude Code with /h/.claude-work to sign in again'},
        {'name': '.codex-work', 'agent': 'codex', 'plan_type': 'prolite',
         'rate_limit': {'primary_window': {'used_percent': 92, 'limit_window_seconds': 604800, 'reset_at': now + 5 * 86400}, 'secondary_window': None}},
    ]})


def html_page(title):
    """The HTML artifact's page under `title`; the check rewrites it to see the pane reload."""
    return f'''<!doctype html><html><meta charset="utf-8"><title>{title}</title>
<style>body{{background:#e9eee8;color:#1c3027;font:18px/1.6 system-ui;padding:22px}}</style>
<h1>{title}</h1><p>Served by the fixture wheeld into the client's WKWebView pane.</p>
<script src=".wheel/client.js"></script><script>wheel.set('seen', {{ok: true, title: document.title}})</script></html>'''


def browser_page(name, title):
    """A file page for the browser pane: it names itself, writes two console lines and has one headline."""
    return f'''<!doctype html><html><meta charset="utf-8"><title>{title}</title>
<style>body{{background:#f4efe6;color:#2b2118;font:20px/1.5 system-ui;padding:24px}}h1{{margin:0 0 12px;font-size:28px}}</style>
<h1 id="headline">{title}</h1><p class="note">A file page the fixture wrote for the browser pane ({name}).</p>
<script>console.log('hello from {name}'); console.warn('careful from {name}');</script></html>'''


@contextlib.contextmanager
def fixture():
    """Yield (env, info). `env` carries WHEEL_URL, WHEEL_TOKEN_FILE and WHEEL_CLIENT_SPACE."""
    env = os.environ.copy()
    for key in ['WHEEL_PANE', 'WHEEL_SPACE', 'WHEEL_SPACE_DIR']:
        env.pop(key, None)
    # A short root under /tmp: the agent footer shows the project's directory and the stand-in's
    # home side by side, and the check's wide pane fits both whole.
    with tempfile.TemporaryDirectory(prefix='wgf-', dir='/tmp') as temp:
        root = pathlib.Path(temp)
        project = root / 'project'
        page = project / 'html'
        page.mkdir(parents=True)
        # The page calls the daemon through `.wheel/client.js`: `wheel artifact state <pane> seen` reads what it stored.
        (page / 'index.html').write_text(html_page('Fixture artifact'))
        pages = root / 'pages'
        pages.mkdir()
        (pages / 'one.html').write_text(browser_page('one', 'Fixture page one'))
        (pages / 'two.html').write_text(browser_page('two', 'Fixture page two'))
        gallery = pathlib.Path(__file__).resolve().parents[3] / 'tools/gallery/server.py'
        (project / 'wheel.toml').write_text(f'[artifacts.fixture-html]\npath="html"\npane=true\n\n[artifacts.fixture-page]\nmcp="python3 {gallery} v17 filters-form"\npane=true\n\n[runner]\nfixture-echo = "echo FIXTURE-RUNNER"\n' + TITLE_PROJECT)
        (root / 'machine.toml').write_text(machine_artifacts(gallery))
        (root / 'usage.json').write_text(usage_readings())
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        url = f'ws://127.0.0.1:{port}/ws'
        token = root / 'support/token'
        for executable in ['wheeld', 'wheel']:
            shutil.copy2(built(executable), root / executable)
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
                '--machine-file', str(root / 'machine.toml'), '--log-dir', str(root / 'logs'), '--usage-fixture', str(root / 'usage.json')], env=env, stdout=log, stderr=log)
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
            cli('config', 'set', 'agent.claude.command', str(built('wheel-fake-agent')), '--machine')
            # The stand-in keeps its conversations where Claude Code would, so it gets a home of its own
            # and nothing the check writes reaches ~/.claude/projects.
            claude_home = root / 'claude-home'
            claude_home.mkdir()
            cli('config', 'set', 'agent.claude.home', str(claude_home), '--machine')
            cli('config', 'set', 'usage.agents', 'claude,codex', '--machine')
            # A Codex pane runs the same stand-in as `codex app-server`, its threads in a home of its own.
            codex_home = root / 'codex-home'
            codex_home.mkdir()
            cli('config', 'set', 'agent.codex.command', str(built('wheel-fake-agent')), '--machine')
            cli('config', 'set', 'agent.codex.home', str(codex_home), '--machine')
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
            # A third session holds one space on this repository with one web pane, which the check
            # moves through every page in `PAGES`.
            pages_session = cli('session', 'new', 'Pages').splitlines()[-1]
            pages_space = cli('space', 'new', os.environ.get('WHEEL_FIXTURE_PAGES_DIR', str(REPO)), '--name', 'pages', '--session', pages_session).splitlines()[-1]
            pages_pane = tree(pages_space)['root']['id']
            cli('pane', 'kind', pages_pane, 'web', '--artifact', PAGES[0], '--scope', 'machine')
            env.update(WHEEL_CLIENT_PAGES_SESSION=pages_session, WHEEL_CLIENT_PAGES_PANE=pages_pane, WHEEL_CLIENT_PAGE_NAMES=json.dumps(PAGES))
            env.update(WHEEL_URL=url, WHEEL_TOKEN_FILE=str(token), WHEEL_CLIENT_SPACE=space, WHEEL_CLIENT_CLI=str(root / 'wheel'), WHEEL_CLIENT_MARKERS=json.dumps(markers),
                       WHEEL_CLIENT_AGENT_PANE=agent_pane, WHEEL_CLIENT_AGENT=started['agent'], WHEEL_CLIENT_HTML=str(page), WHEEL_CLIENT_PAGES=str(pages))
            info = dict(space=space, daemon_pid=daemon.pid, port=port, panes=dict(web=web, terminal=term_pane, browser=browser, terminal2=term2_pane, web2=web2, agent=agent_pane),
                        root=root, claude_home=claude_home, saved_tree=lambda: tree(space)['root'], daemon_alive=lambda: running[-1].poll() is None, restart=restart)
            yield env, info
        finally:
            stop()
            log.close()
