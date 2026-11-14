#!/usr/bin/env python3
"""Build the client: the Rust binaries, the Swift bridge, the bundled server, and the app bundle.

    build.py [release|dev]

`release` (the default) makes `target/app/Wheel.app`, id `com.piercemakes.wheel`, carrying the
release wheeld as `com.piercemakes.wheel.daemon` on 7420. `dev` makes `target/app/Wheel Dev.app`, id
`com.piercemakes.wheel.dev`, with the DEV icon and the debug wheeld as
`com.piercemakes.wheel.dev.daemon` on 7422. These are the ids macOS holds the permissions and login
items under, so `make deploy-client` and `make deploy-dev` install these bundles as
`/Applications/Wheel.app` and `/Applications/Wheel Dev.app` (ADR-0040).

The bridge links Wheel's already-built release objects (GhosttyTerminal, WheelKit, WheelUI and what
they use) from DerivedData, so the Swift Release build runs first (`tools/install-app.sh` does it,
or a Release xcodebuild into `DerivedData-deploy-release.noindex`). WHEEL_NATIVE_PRODUCTS selects
another product directory. CARGO_TARGET_DIR is honoured for the client's own crates.

The server goes in the same way as the Mac app's: `apple/Mac/embed-wheeld.sh`, the Xcode build
phase, run with the variables Xcode would set (CONFIGURATION Release or Debug picks the variant),
builds the repo's wheeld, `wheel` and `wheel-runner` (into the repo's own target directory) and
puts `Contents/Helpers/<server name>.wheelserver`, `Helpers/wheel`, `Helpers/wheel-runner`,
`Resources/wheel-server.sh` and `Resources/WheelDaemon.plist` in the bundle (CLAUDE.md,
"Deploying Wheel from inside Wheel").

Every binary and the bundle are signed with the keychain's Apple Development certificate
(`tools/signing-identity`), the identity the Swift apps carry, so macOS keeps the permissions it
granted across rebuilds; with no certificate the signature is ad hoc.
"""
import os, pathlib, plistlib, shutil, subprocess, sys, tempfile

VARIANTS = dict(
    release=dict(name='Wheel', bundle_id='com.piercemakes.wheel', configuration='Release', icon='AppIcon'),
    dev=dict(name='Wheel Dev', bundle_id='com.piercemakes.wheel.dev', configuration='Debug', icon='AppIconDev'),
)
variant = sys.argv[1] if len(sys.argv) > 1 else 'release'
if variant not in VARIANTS or len(sys.argv) > 2:
    sys.exit('usage: build.py [release|dev]')
spec = VARIANTS[variant]

here = pathlib.Path(__file__).resolve().parent
client = here.parent
repo = client.parents[1]
products = pathlib.Path(os.environ.get('WHEEL_NATIVE_PRODUCTS', repo / 'DerivedData-deploy-release.noindex/Build/Products/Release'))
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

subprocess.run(['cargo', '+stable', 'build', '--release', '--manifest-path', str(client / 'Cargo.toml')], check=True)
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
    # The Swift app's menu bar image: the mark with the badge of spaces no window shows.
    str(repo / 'apple/Mac/Sources/MenuBarIcon.swift'),
    *[str(o) for o in sorted(products.glob('*.o'))],
    str(products / 'libghostty.a'), '-lc++', '-framework', 'Carbon', '-framework', 'Metal', '-framework', 'WebKit',
    '-o', str(contents / 'MacOS/libWheelGpuiBridge.dylib')], check=True)
sign(contents / 'MacOS/libWheelGpuiBridge.dylib')
for name in ['wheel-gpui', 'wheel-gpui-check']:
    shutil.copy2(target / 'release' / name, fresh(contents / 'MacOS' / name))
    sign(contents / 'MacOS' / name)
for resource in products.glob('*.bundle'):
    shutil.copytree(resource, contents / 'Resources' / resource.name, dirs_exist_ok=True)

# The icon is the Swift app's asset set (the DEV band for the dev variant), folded into an .icns:
# the set's sizes are each iconset entry and its @2x.
iconset_src = repo / 'apple/Mac/Assets.xcassets' / f"{spec['icon']}.appiconset"
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
    NSHighResolutionCapable=True)))
subprocess.run(['bash', str(repo / 'apple/Mac/embed-wheeld.sh')], check=True, env=dict(os.environ,
    CONFIGURATION=spec['configuration'], SRCROOT=str(repo / 'apple'), TARGET_BUILD_DIR=str(bundle.parent),
    CONTENTS_FOLDER_PATH=f'{bundle.name}/Contents', EXPANDED_CODE_SIGN_IDENTITY=identity))
# The bundle is signed last, after every file inside it is in place and signed.
sign(bundle)
print(bundle)
