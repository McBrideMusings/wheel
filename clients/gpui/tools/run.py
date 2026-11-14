#!/usr/bin/env python3
"""Open the client on screen against a throwaway wheeld (a four-pane space), until its window closes.

`run.py --real` connects to the running wheeld instead; a divider drag then resizes your real layout.
"""
import pathlib, subprocess, sys

here = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(here))
from fixture import fixture  # noqa: E402

app = here.parent / 'target/app/Wheel.app/Contents/MacOS'
client = [str(app / 'wheel-gpui')]
if '--real' in sys.argv:
    subprocess.run(client, env={**__import__('os').environ, 'WHEEL_GPUI_BRIDGE': str(app / 'libWheelGpuiBridge.dylib')})
else:
    with fixture() as (env, info):
        env['WHEEL_GPUI_BRIDGE'] = str(app / 'libWheelGpuiBridge.dylib')
        print(f"fixture wheeld pid {info['daemon_pid']} on port {info['port']}; close the window to stop it", flush=True)
        subprocess.run(client + ['--space', info['space']], env=env)
