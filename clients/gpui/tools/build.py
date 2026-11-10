#!/usr/bin/env python3
"""Build the client: the Rust binaries, the Swift bridge, and `target/app/Wheel GPUI.app`.

The bridge links Wheel's already-built release GhosttyTerminal objects from DerivedData, so
build the Mac app first (`make deploy-client` or a Release xcodebuild). WHEEL_NATIVE_PRODUCTS
selects another product directory. CARGO_TARGET_DIR is honoured.
"""
import os, pathlib, plistlib, shutil, subprocess

here = pathlib.Path(__file__).resolve().parent
client = here.parent
repo = client.parents[1]
products = pathlib.Path(os.environ.get('WHEEL_NATIVE_PRODUCTS', repo / 'DerivedData-deploy-release.noindex/Build/Products/Release'))
headers = repo / 'DerivedData/SourcePackages/artifacts/libghostty-spm/libghostty/GhosttyKit.xcframework/macos-arm64_x86_64/Headers'
target = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', client / 'target'))
contents = client / 'target/app/Wheel GPUI.app/Contents'
(contents / 'MacOS').mkdir(parents=True, exist_ok=True)
(contents / 'Resources').mkdir(exist_ok=True)

subprocess.run(['cargo', '+stable', 'build', '--release', '--manifest-path', str(client / 'Cargo.toml')], check=True)
subprocess.run(['turnstile', 'run', '--', 'xcrun', 'swiftc', '-O', '-emit-library', '-module-name', 'WheelGpuiBridge',
    '-I', str(products), '-I', str(headers), str(client / 'native/Bridge.swift'),
    *[str(products / (name + '.o')) for name in ['GhosttyTerminal', 'GhosttyKit', 'DisplayLink']],
    str(products / 'libghostty.a'), '-lc++', '-framework', 'Carbon', '-framework', 'Metal', '-framework', 'WebKit',
    '-o', str(contents / 'MacOS/libWheelGpuiBridge.dylib')], check=True)
for name in ['wheel-gpui', 'wheel-gpui-check']:
    shutil.copy2(target / 'release' / name, contents / 'MacOS' / name)
shutil.copytree(products / 'GhosttyKit_GhosttyTerminal.bundle', contents / 'Resources/GhosttyKit_GhosttyTerminal.bundle', dirs_exist_ok=True)
(contents / 'Info.plist').write_bytes(plistlib.dumps(dict(
    CFBundleExecutable='wheel-gpui', CFBundleIdentifier='com.piercemakes.wheel.gpui', CFBundleName='Wheel GPUI',
    CFBundlePackageType='APPL', NSHighResolutionCapable=True)))
print(contents.parent)
