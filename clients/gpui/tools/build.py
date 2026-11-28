#!/usr/bin/env python3
"""Build the client: the Rust binaries, the Swift bridge, the bundled server, and the app bundle.

    build.py [release|dev] [--check]

`release` (the default) makes `target/app/Wheel.app`, id `com.piercemakes.wheel`, carrying the
release wheeld as `com.piercemakes.wheel.daemon` on 7420. `dev` makes `target/app/Wheel Dev.app`, id
`com.piercemakes.wheel.dev`, with the DEV icon and the debug wheeld as
`com.piercemakes.wheel.dev.daemon` on 7422. These are the ids macOS holds the permissions and login
items under, so `make deploy-client` and `make deploy-dev` install these bundles as
`/Applications/Wheel.app` and `/Applications/Wheel Dev.app` (ADR-0040).

The bridge links Wheel's release objects (GhosttyTerminal, WheelKit, WheelUI and what they use):
`tools/swift-objects.sh` builds them from this checkout into `DerivedData-deploy-release.noindex`
first, its output in `target/swift-objects.log`. WHEEL_NATIVE_PRODUCTS names a product directory a
caller built already (`tools/install-app.sh` does), and skips that build. CARGO_TARGET_DIR is
honoured for the client's own crates.

The server goes in the same way as the Mac app's: `apple/Mac/embed-wheeld.sh`, the Xcode build
phase, run with the variables Xcode would set (CONFIGURATION Release or Debug picks the variant),
builds the repo's wheeld, `wheel` and `wheel-runner` (into the repo's own target directory) and
puts `Contents/Helpers/<server name>.wheelserver`, `Helpers/wheel`, `Helpers/wheel-runner`,
`Resources/wheel-server.sh` and `Resources/WheelDaemon.plist` in the bundle (CLAUDE.md,
"Deploying Wheel from inside Wheel").

Every binary and the bundle are signed with the keychain's Apple Development certificate
(`tools/signing-identity`), the identity the Swift apps carry, so macOS keeps the permissions it
granted across rebuilds; with no certificate the signature is ad hoc.

`--check` also builds the hidden regression check, `wheel-gpui-check`, and makes
`target/app/check/<name>.app`: a copy of the bundle with the check beside `wheel-gpui`, signed again.
The check runs from inside a bundle because it finds the bridge and the server through its own
path; the bundle that is installed never carries it.
"""
import os, pathlib, plistlib, shutil, subprocess, sys, tempfile

VARIANTS = dict(
    release=dict(name='Wheel', bundle_id='com.piercemakes.wheel', configuration='Release', icon='AppIcon'),
    dev=dict(name='Wheel Dev', bundle_id='com.piercemakes.wheel.dev', configuration='Debug', icon='AppIconDev'),
)
args = sys.argv[1:]
with_check = '--check' in args
args = [a for a in args if a != '--check']
variant = args[0] if args else 'release'
if variant not in VARIANTS or len(args) > 1:
    sys.exit('usage: build.py [release|dev] [--check]')
spec = VARIANTS[variant]

here = pathlib.Path(__file__).resolve().parent
client = here.parent
repo = client.parents[1]
if 'WHEEL_NATIVE_PRODUCTS' in os.environ:
    products = pathlib.Path(os.environ['WHEEL_NATIVE_PRODUCTS'])
else:
    # Objects left by an earlier build may predate this checkout's Swift (a protocol level the bridge then refuses).
    swift_log = client / 'target/swift-objects.log'
    if subprocess.run([str(repo / 'tools/swift-objects.sh'), str(swift_log)]).returncode:
        sys.exit(f'the Swift Release objects failed to build; see {swift_log}')
    products = repo / 'DerivedData-deploy-release.noindex/Build/Products/Release'
# The libghostty headers come from the same build as the objects linked: <derived data>/Build/Products/Release.
headers = products.parents[2] / 'SourcePackages/artifacts/libghostty-spm/libghostty/GhosttyKit.xcframework/macos-arm64_x86_64/Headers'
target = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', client / 'target'))
bundle = client / 'target/app' / f"{spec['name']}.app"
contents = bundle / 'Contents'
(contents / 'MacOS').mkdir(parents=True, exist_ok=True)
(contents / 'Resources').mkdir(exist_ok=True)
identity = subprocess.run([str(repo / 'tools/signing-identity')], check=True, capture_output=True, text=True).stdout.strip() or '-'
if identity == '-':
    print('warning: no Apple Development certificate in the keychain; signing ad hoc', file=sys.stderr)

packages = ['-p', 'wheel-gpui'] + (['-p', 'wheel-gpui-check'] if with_check else [])
subprocess.run(['cargo', '+stable', 'build', '--release', *packages, '--manifest-path', str(client / 'Cargo.toml')], check=True)
# A binary written over one macOS has already run is killed at its next launch ("load code
# signature error 2": the mapped pages no longer match the signature), so each is replaced, never
# overwritten, and signed again.
def fresh(path):
    path.unlink(missing_ok=True)
    return path
def sign(path):
    # codesign reports "replacing existing signature" on stderr for every rebuild; only a failure is shown.
    signed = subprocess.run(['codesign', '--force', '--sign', identity, str(path)], capture_output=True, text=True)
    if signed.returncode:
        sys.exit(f'codesign of {path} failed: {signed.stderr.strip()}')
fresh(contents / 'MacOS/libWheelGpuiBridge.dylib')
subprocess.run(['turnstile', 'run', '--', 'xcrun', 'swiftc', '-O', '-emit-library', '-module-name', 'WheelGpuiBridge',
    '-I', str(products), '-I', str(headers),
    # WheelUI's PluginPane uses WheelLuau, whose C modules (the Luau headers) the compiler still loads from WheelUI's module;
    # Xcode wrote their module maps beside the objects.
    *[arg for name in ['CLuau', 'LuauCore'] for arg in ['-Xcc', f'-fmodule-map-file={products.parents[1] / "Intermediates.noindex/GeneratedModuleMaps" / (name + ".modulemap")}']],
    '-Xcc', '-I' + str(repo / 'apple/WheelLuau/luau/VM/include'),
    str(client / 'native/Bridge.swift'),
    # The Swift app's own source for how `terminal.*` settings become a Ghostty configuration.
    str(repo / 'apple/Shared/TerminalStyle.swift'),
    # WheelUI's browser pieces are the iPhone's; the pane host makes the Mac's browsers in Rust (web.rs).
    *[str(o) for o in sorted(products.glob('*.o')) if o.stem not in ('BrowserPane', 'BrowserInspect', 'BrowserConsole')],
    str(products / 'libghostty.a'), '-lc++', '-framework', 'Carbon', '-framework', 'Metal', '-framework', 'WebKit',
    '-o', str(contents / 'MacOS/libWheelGpuiBridge.dylib')], check=True)
sign(contents / 'MacOS/libWheelGpuiBridge.dylib')
shutil.copy2(target / 'release/wheel-gpui', fresh(contents / 'MacOS/wheel-gpui'))
sign(contents / 'MacOS/wheel-gpui')
# A bundle an earlier build made may still hold the check.
fresh(contents / 'MacOS/wheel-gpui-check')
for resource in products.glob('*.bundle'):
    shutil.copytree(resource, contents / 'Resources' / resource.name, dirs_exist_ok=True)

# The icon is the asset set beside the client (the DEV band for the dev variant), folded into an .icns:
# the set's sizes are each iconset entry and its @2x.
iconset_src = client / 'Assets.xcassets' / f"{spec['icon']}.appiconset"
with tempfile.TemporaryDirectory() as tmp:
    iconset = pathlib.Path(tmp) / 'AppIcon.iconset'
    iconset.mkdir()
    for size in [16, 32, 128, 256, 512]:
        shutil.copy2(iconset_src / f'{size}.png', iconset / f'icon_{size}x{size}.png')
        shutil.copy2(iconset_src / f'{size * 2}.png', iconset / f'icon_{size}x{size}@2x.png')
    subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(fresh(contents / 'Resources/AppIcon.icns'))], check=True)

(contents / 'Info.plist').write_bytes(plistlib.dumps(dict(
    CFBundleDevelopmentRegion='en', CFBundleExecutable='wheel-gpui', CFBundleIdentifier=spec['bundle_id'],
    CFBundleName=spec['name'], CFBundleDisplayName=spec['name'], CFBundlePackageType='APPL',
    CFBundleInfoDictionaryVersion='6.0', CFBundleIconFile='AppIcon', CFBundleSupportedPlatforms=['MacOSX'],
    LSApplicationCategoryType='public.app-category.developer-tools', LSMinimumSystemVersion='15.0',
    NSHighResolutionCapable=True,
    NSMicrophoneUsageDescription='Wheel turns what you say into the text of a message, on this Mac. Your voice never leaves it.',
    # A browser pane opens whatever address it is given, plain http on a Tailscale or LAN host included.
    NSAppTransportSecurity=dict(NSAllowsArbitraryLoadsInWebContent=True))))
subprocess.run(['bash', str(repo / 'apple/Mac/embed-wheeld.sh')], check=True, env=dict(os.environ,
    CONFIGURATION=spec['configuration'], SRCROOT=str(repo / 'apple'), TARGET_BUILD_DIR=str(bundle.parent),
    CONTENTS_FOLDER_PATH=f'{bundle.name}/Contents', EXPANDED_CODE_SIGN_IDENTITY=identity))
# The bundle is signed last, after every file inside it is in place and signed.
sign(bundle)
if with_check:
    checked = client / 'target/app/check' / bundle.name
    shutil.rmtree(checked, ignore_errors=True)
    checked.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['ditto', str(bundle), str(checked)], check=True)
    shutil.copy2(target / 'release/wheel-gpui-check', checked / 'Contents/MacOS/wheel-gpui-check')
    sign(checked / 'Contents/MacOS/wheel-gpui-check')
    sign(checked)
print(bundle)
