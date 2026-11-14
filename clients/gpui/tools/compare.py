#!/usr/bin/env python3
"""Side by side numbers for the two Mac clients against one fixture wheeld (wheel-ynjo.19).

    compare.py [--reps 3] [--clients swift,gpui] [--out DIR] [--phases startup,memory,cpu,frames]

One fixture wheeld (`fixture.py`) serves both clients, one run after the other, interleaved
(swift, gpui, swift, gpui, ...). Every run builds its own space on that wheeld: two agent panes
(the fake agent), a terminal, a popped-out terminal, an artifact pane and a browser pane. The
client opens hidden (Swift: `WHEEL_DEBUG_PORT` test mode; GPUI: `--hidden`), connects to it, and
the script reads, per run:

  startup   process launch to a connected client whose space is drawn (see `ready_*`)
  memory    RSS (ps) and phys_footprint (footprint) idle after settle, after 40 replies, at the end
  cpu       idle mean over 10 s, and mean over the last 20 s of 36 s in which the agent replies
            every 2 s while ten characters a second are typed into its composer (perf-chat.sh)
  frames    a 2.5 s window resize sweep: Swift reports `/metrics` (display-link gaps, main-thread
            CPU), GPUI reports `/measure/sweep` (the time of each `Window::draw`, one per 16 ms step)

Each reading carries `uptime`'s load averages. Needs the repo's debug wheeld, `wheel`, wheel-fake-agent,
wheelhouse and wheel-runner (`cargo build -p wheeld -p wheel-cli -p wheelhouse -p wheel-runner`), the
GPUI app (`python3 clients/gpui/tools/build.py`) and the optimized Debug Mac app (`make compare-swift`).
Writes results.json and results.md under --out. Never touches the installed Wheel.app or its wheeld.
"""
import argparse, json, os, pathlib, re, statistics, subprocess, sys, threading, time, urllib.parse, urllib.request

here = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(here))
from fixture import fixture  # noqa: E402

REPO = here.parents[2]
GPUI_APP = here.parent / 'target/app/Wheel.app'
# The Swift app's debug server is compiled only into its Debug configuration, so the app measured is
# that configuration built optimized (`make compare-swift`), set against the optimized GPUI build.
SWIFT_APP = REPO / 'DerivedData-compare-opt.noindex/Build/Products/Debug/Wheel.app'
MSG = '''## Reply N

Prose with `inline code`, a path apple/Mac/Sources/AppModel.swift:120, a missing one missing/fileN.swift:9 and **bold** text long enough to wrap across lines in a narrow pane.

- an item with a link https://example.com/a/b
- an item with `code`

```swift
let x = 1
print(x + 2)
```

| a | b |
|---|---|
| 1 | 2 |'''


def load():
    return subprocess.run(['uptime'], capture_output=True, text=True).stdout.split('load averages:')[-1].strip()


def median_range(values):
    values = [v for v in values if v is not None]
    if not values:
        return None
    return dict(median=round(statistics.median(values), 2), min=round(min(values), 2), max=round(max(values), 2), n=len(values))


class Client:
    """A running client: its process, its debug port, and HTTP to it."""
    def __init__(self, kind, env, space):
        self.kind, self.env, self.space = kind, env, space
        self.port = None
        self.proc = None
        self.token_file = env['WHEEL_TOKEN_FILE']

    def http(self, method, path, timeout=30, **query):
        url = f'http://127.0.0.1:{self.port}{path}' + ('?' + urllib.parse.urlencode(query) if query else '')
        request = urllib.request.Request(url, method=method, data=b'' if method == 'POST' else None,
                                         headers={'Authorization': 'Bearer ' + pathlib.Path(self.token_file).read_text().strip()})
        with urllib.request.urlopen(request, timeout=timeout) as reply:
            return json.loads(reply.read() or b'{}')

    def state(self):
        return self.http('GET', '/state')

    def start(self, log):
        s = socket_free_port()
        self.port = s
        env = dict(self.env, WHEEL_DEBUG_PORT=str(s))
        if self.kind == 'gpui':
            env['WHEEL_GPUI_BRIDGE'] = str(GPUI_APP / 'Contents/MacOS/libWheelGpuiBridge.dylib')
            command = [str(GPUI_APP / 'Contents/MacOS/wheel-gpui'), '--hidden', '--space', self.space]
        else:
            env['WHEEL_PREFS_SUITE'] = f'wheel.compare.{os.getpid()}.{s}'
            self.suite = env['WHEEL_PREFS_SUITE']
            command = [str(SWIFT_APP / 'Contents/MacOS/Wheel')]
        self.started = time.monotonic()
        self.proc = subprocess.Popen(command, env=env, stdout=log, stderr=log)

    @property
    def pid(self):
        return self.proc.pid

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.kind == 'swift' and getattr(self, 'suite', None):
            subprocess.run(['defaults', 'delete', self.suite], capture_output=True)


def socket_free_port():
    import socket
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def ps(pid):
    """(rss in MB, cpu seconds) of the process."""
    out = subprocess.run(['ps', '-o', 'rss=,time=', '-p', str(pid)], capture_output=True, text=True).stdout.split()
    seconds = 0.
    for part in out[1].split(':'):
        seconds = seconds * 60 + float(part)
    return int(out[0]) / 1024, seconds


def footprint_mb(pid, scratch):
    path = scratch / f'fp-{pid}.json'
    subprocess.run(['footprint', str(pid), '-j', str(path)], capture_output=True)
    return json.loads(path.read_text())['processes'][0]['footprint'] / 1048576


def memory(client, scratch):
    rss, _ = ps(client.pid)
    return dict(rss_mb=round(rss, 1), footprint_mb=round(footprint_mb(client.pid, scratch), 1), load=load())


def cpu_mean(client, seconds):
    """Mean CPU over `seconds` as a percentage of one core, from the process's cputime."""
    _, a = ps(client.pid)
    t0 = time.monotonic()
    time.sleep(seconds)
    _, b = ps(client.pid)
    return (b - a) * 100 / (time.monotonic() - t0)


def ready_gpui(client, agent_pane):
    s = client.state()
    pane = s['panes'].get(agent_pane)
    return s['connection'] == 'connected' and s.get('frames', 0) >= 1 and bool(pane and pane.get('visible'))


def ready_swift(client, agent_pane):
    s = client.state()
    return s['connection'] == 'connected' and agent_pane in s.get('panes', {}) and s['active'] == client.space if 'active' in s else False


def startup(client, agent_pane, timeout=60):
    """Seconds from launch to connected and the measured space's agent pane drawn."""
    focused = False
    while time.monotonic() - client.started < timeout:
        if client.proc.poll() is not None:
            raise RuntimeError(f'{client.kind} exited with {client.proc.returncode}')
        try:
            if client.kind == 'swift':
                s = client.state()
                if s.get('connection') == 'connected' and not focused:
                    client.http('POST', '/focus', space=client.space)
                    focused = True
                    continue
                if focused and wait_swift_drawn(s, client, agent_pane):
                    return time.monotonic() - client.started
            elif ready_gpui(client, agent_pane):
                return time.monotonic() - client.started
        except Exception:
            pass
        time.sleep(0.02)
    raise TimeoutError(f'{client.kind} did not show the space within {timeout} s')


def wait_swift_drawn(state, client, agent_pane):
    pane = state.get('panes', {}).get(agent_pane)
    return bool(pane) and state.get('connection') == 'connected' and pane.get('visible', True) is not False and pane.get('frame', pane.get('rect', True)) not in (None, [0, 0, 0, 0])


def run_cli(env, *args, timeout=60):
    return subprocess.check_output([env['WHEEL_CLIENT_CLI'], '--url', env['WHEEL_URL'], '--token-file', env['WHEEL_TOKEN_FILE'], *args], env=env, text=True, timeout=timeout).strip()


def make_space(env, root, n):
    """A space on the fixture wheeld: artifact, terminal, popped terminal, browser, two agents."""
    cli = lambda *a, **k: run_cli(env, *a, **k)  # noqa: E731
    space = cli('space', 'new', str(root / 'project'), '--name', f'compare-{n}').splitlines()[-1]
    tree = next(s for s in json.loads(cli('status', '--json'))['spaces'] if s['id'] == space)
    web = tree['root']['id']
    cli('pane', 'kind', web, 'web', '--artifact', 'fixture-html')
    term = cli('pane', 'split', web, 'right').splitlines()[-1]
    cli('pane', 'kind', term, 'terminal')
    browser = cli('pane', 'split', term, 'down').splitlines()[-1]
    cli('pane', 'kind', browser, 'browser', '--page', 'about:blank')
    agent = cli('pane', 'split', web, 'down').splitlines()[-1]
    cli('pane', 'start', agent, 'agent', '--agent', 'claude')
    agent2 = cli('pane', 'split', agent, 'right').splitlines()[-1]
    cli('pane', 'start', agent2, 'agent', '--agent', 'claude')
    popped = cli('pane', 'split', term, '--pop').splitlines()[-1]
    cli('pane', 'kind', popped, 'terminal')
    return dict(space=space, agent=agent, agent2=agent2, popped=popped, panes=[web, term, browser, agent, agent2, popped])


def replies(env, agent, first, count):
    for i in range(first, first + count):
        run_cli(env, 'agent', 'send', agent, MSG.replace('N', str(i)), '--timeout', '60', timeout=90)


def activity(client, env, agent, agent_session, seconds=36):
    """For `seconds`: the agent replies every 2 s and ten characters a second are typed into its composer.
    Returns the mean CPU over the last 20 s, with a one second sample each."""
    stop = threading.Event()

    def reply_loop():
        i = 0
        while not stop.is_set():
            i += 1
            t = time.monotonic()
            try:
                run_cli(env, 'agent', 'send', agent, MSG.replace('N', f'live {i}'), '--timeout', '30', timeout=40)
            except Exception:
                pass
            stop.wait(max(0, 2 - (time.monotonic() - t)))

    def type_loop():
        if client.kind == 'swift':
            client.http('POST', '/agent/typing', agent=agent_session, chars=int(seconds * 10), per_second=10)
            return
        client.http('POST', '/agent/focus', agent=agent_session)
        for n in range(int(seconds * 10)):
            if stop.is_set():
                return
            client.http('POST', '/type', text=' ' if n % 6 == 5 else 'a')
            stop.wait(0.1)
    threads = [threading.Thread(target=reply_loop), threading.Thread(target=type_loop)]
    for t in threads:
        t.start()
    time.sleep(seconds - 20)
    mean = cpu_mean(client, 20)
    stop.set()
    for t in threads:
        t.join()
    return mean


def frames(client):
    if client.kind == 'gpui':
        report = client.http('POST', '/measure/sweep', timeout=60, seconds=2.5, by=300)
        gap, draw = report['frame_gap_ms'], report['draw_ms']
        return dict(kind='gap between frames (one Window::draw per 16 ms resize step)', frames=report['frames'], wall_s=round(report['wall_s'], 2),
                    p50_ms=round(gap['median'], 2), p95_ms=round(gap['p95'], 2), max_ms=round(gap['max'], 2), over_34ms=gap['over_34'],
                    draw_p50_ms=round(draw['median'], 2), draw_p95_ms=round(draw['p95'], 2), draw_max_ms=round(draw['max'], 2))
    window = client.state()['windows'][0]['id']
    client.http('POST', '/metrics', action='start')
    client.http('POST', '/windows', action='sweep', window=window, seconds=2.5, by=300)
    time.sleep(3.3)
    report = client.http('POST', '/metrics', action='stop')
    f = report['frame_ms']
    return dict(kind='display-link gap per frame (GridMetrics, main thread)', frames=report['frames'], duration_ms=report['duration_ms'], p50_ms=f['p50'], p95_ms=f['p95'], max_ms=f['max'], over_34ms=f['over_34ms'], main_cpu_ms=report['main_cpu_ms'])


def one_run(kind, rep, env, info, scratch, phases):
    n = len(list(scratch.glob('run-*.log')))
    space = make_space(env, info['root'], n)
    run = dict(client=kind, rep=rep, load_at_start=load())
    env = dict(env, WHEEL_CLIENT_SPACE=space['space'])
    client = Client(kind, env, space['space'])
    with (scratch / f'run-{n}-{kind}.log').open('w') as log:
        try:
            client.start(log)
            run['startup_s'] = round(startup(client, space['agent']), 3)
            run['load_startup'] = load()
            state = client.state()
            session = state['panes'][space['agent']]['agent']
            run['pid'] = client.pid
            time.sleep(8)
            if 'memory' in phases:
                run['mem_idle'] = memory(client, scratch)
            if 'cpu' in phases:
                run['cpu_idle_pct'] = round(cpu_mean(client, 10), 2)
                run['load_idle'] = load()
            replies(env, space['agent'], 1, 40)
            time.sleep(5)
            if 'memory' in phases:
                run['mem_40_replies'] = memory(client, scratch)
            if 'cpu' in phases:
                run['cpu_busy_pct'] = round(activity(client, env, space['agent'], session), 2)
                run['load_busy'] = load()
            if 'frames' in phases:
                run['frames'] = frames(client)
                run['load_frames'] = load()
            if 'memory' in phases:
                time.sleep(3)
                run['mem_end'] = memory(client, scratch)
            if client.kind == 'swift':
                run['swift_requests'] = {k: v for k, v in json.loads(run_cli(env, 'status', '--json')).get('requests', {}).items() if k.startswith('mac ')}
        finally:
            client.stop()
            try:
                run_cli(env, 'space', 'close', space['space'])
            except Exception:
                pass
    return run


def du(path):
    return int(subprocess.run(['du', '-sk', str(path)], capture_output=True, text=True).stdout.split()[0]) / 1024


def sizes():
    out = {}
    swift_release = REPO / 'DerivedData-deploy-release.noindex/Build/Products/Release/Wheel.app'
    for name, app, binary in [('swift_measured_debug_O', SWIFT_APP, 'Contents/MacOS/Wheel'), ('swift_release', swift_release, 'Contents/MacOS/Wheel'),
                              ('gpui_release', GPUI_APP, 'Contents/MacOS/wheel-gpui')]:
        if app.exists():
            exe = app / binary
            out[name] = dict(app=str(app), bundle_mb=round(du(app), 1), binary_mb=round(exe.stat().st_size / 1048576, 1),
                             debug_dylib_mb=round(sum(p.stat().st_size for p in app.glob('Contents/MacOS/*.debug.dylib')) / 1048576, 1) or None,
                             bridge_dylib_mb=round(p.stat().st_size / 1048576, 1) if (p := app / 'Contents/MacOS/libWheelGpuiBridge.dylib').exists() else None)
    return out


METRICS = [('startup (s)', 'startup_s'), ('RSS idle (MB)', 'mem_idle.rss_mb'), ('footprint idle (MB)', 'mem_idle.footprint_mb'),
           ('RSS after 40 replies (MB)', 'mem_40_replies.rss_mb'), ('footprint after 40 replies (MB)', 'mem_40_replies.footprint_mb'),
           ('RSS end (MB)', 'mem_end.rss_mb'), ('footprint end (MB)', 'mem_end.footprint_mb'),
           ('CPU idle, mean of 10 s (%)', 'cpu_idle_pct'), ('CPU while replying and typing (%)', 'cpu_busy_pct'),
           ('resize frame p50 (ms)', 'frames.p50_ms'), ('resize frame p95 (ms)', 'frames.p95_ms'), ('resize frame max (ms)', 'frames.max_ms'),
           ('resize frames over 34 ms', 'frames.over_34ms'), ('resize frames recorded', 'frames.frames')]


def dig(run, dotted):
    value = run
    for part in dotted.split('.'):
        if not isinstance(value, dict) or part not in value:
            return None
        value = value[part]
    return value


def summarize(runs, kinds):
    return {kind: {label: median_range([dig(r, key) for r in runs if r['client'] == kind]) for label, key in METRICS} for kind in kinds}


def markdown(report, kinds):
    lines = [f"# Swift vs GPUI, one fixture wheeld (wheel-ynjo.19)", '', f"Run at {report['started']}; {report['reps']} runs per client, interleaved. Cells are median (min-max). Machine load averages per run are in results.json.", '']
    lines += ['| measure | ' + ' | '.join(kinds) + ' |', '|---|' + '---|' * len(kinds)]
    for label, _ in METRICS:
        cells = []
        for kind in kinds:
            m = report['summary'][kind][label]
            cells.append('not measured' if not m else f"{m['median']} ({m['min']}-{m['max']})")
        lines.append(f'| {label} | ' + ' | '.join(cells) + ' |')
    lines += ['', '## Runs', '', '| client | rep | load at start | load at idle CPU | load while busy | load at frames |', '|---|---|---|---|---|---|']
    for r in report['runs']:
        lines.append(f"| {r['client']} | {r['rep']} | {r.get('load_at_start')} | {r.get('load_idle')} | {r.get('load_busy')} | {r.get('load_frames')} |")
    lines += ['', '## Sizes', '', '| build | bundle (MB) | main binary (MB) | bridge dylib (MB) |', '|---|---|---|---|']
    for name, s in report['sizes'].items():
        lines.append(f"| {name} | {s['bundle_mb']} | {s['binary_mb']} | {s.get('bridge_dylib_mb') or ''} |")
    return '\n'.join(lines) + '\n'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--clients', default='swift,gpui')
    ap.add_argument('--phases', default='startup,memory,cpu,frames')
    ap.add_argument('--out', default='/private/tmp/claude/wheel/compare')
    args = ap.parse_args()
    kinds, phases = args.clients.split(','), set(args.phases.split(','))
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    scratch = out / 'scratch'
    scratch.mkdir(exist_ok=True)
    report = dict(started=time.strftime('%Y-%m-%d %H:%M:%S %z'), reps=args.reps, runs=[], load_at_start=load())
    with fixture() as (env, info):
        report['fixture'] = dict(daemon_pid=info['daemon_pid'], port=info['port'])
        for rep in range(1, args.reps + 1):
            for kind in kinds:
                print(f'run {kind} {rep}/{args.reps}  load {load()}', flush=True)
                try:
                    run = one_run(kind, rep, env, info, scratch, phases)
                except Exception as error:  # a failed run is recorded, never dropped
                    run = dict(client=kind, rep=rep, error=f'{type(error).__name__}: {error}', load_at_start=load())
                    print('  failed:', run['error'], flush=True)
                report['runs'].append(run)
                (out / 'results.json').write_text(json.dumps(report, indent=2))
    report['sizes'] = sizes()
    report['summary'] = summarize(report['runs'], kinds)
    (out / 'results.json').write_text(json.dumps(report, indent=2))
    (out / 'results.md').write_text(markdown(report, kinds))
    print((out / 'results.md').read_text())


if __name__ == '__main__':
    main()
