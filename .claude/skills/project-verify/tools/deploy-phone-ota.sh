#!/usr/bin/env bash
# Builds the WheelPhone app as a development-signed .ipa and serves it over the tailnet with
# `tailscale serve`, so the phone installs it from anywhere on the tailnet: open the printed page
# in Safari on the phone and tap Install. The script waits until the phone has downloaded the
# package (WHEEL_PHONE_OTA_WAIT seconds at most, 1800 by default), then takes the page down.
# Needs IOS_DEVELOPMENT_TEAM (read from ~/.claude/.env when not already set); the phone must be
# registered on that team, which `make deploy-phone` does once.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
[ -f "$HOME/.claude/.env" ] && { set -a; . "$HOME/.claude/.env"; set +a; }
BUNDLE=com.piercemakes.wheel.phone
[ -n "${IOS_DEVELOPMENT_TEAM:-}" ] || { echo "set IOS_DEVELOPMENT_TEAM (the Apple team id)" >&2; exit 2; }
DD="${WHEEL_IOS_DERIVED_DATA:-$root/DerivedData-phone}"
SERVE_PATH=/wheel-phone
PORT="${WHEEL_PHONE_OTA_PORT:-7431}"
WAIT="${WHEEL_PHONE_OTA_WAIT:-1800}"
# `debugging` is a development-signed export: it installs on the team's registered devices.
METHOD="${WHEEL_PHONE_OTA_METHOD:-debugging}"
work="$(mktemp -d "${TMPDIR:-/tmp}/wheel-phone-ota.XXXXXX")"
SERVE="$work/serve"
server=""
cleanup() {
  [ -n "$server" ] && { kill "$server" 2>/dev/null; tailscale serve --set-path "$SERVE_PATH" off >/dev/null 2>&1; }
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
# The build number is the build's UTC time, so every install is newer than the one before it.
stamp="$(date -u +%Y%m%d%H%M%S)"

host="$(tailscale status --json | jq -r '.Self.DNSName | rtrimstr(".")')"
[ -n "$host" ] && [ "$host" != null ] || { echo "tailscale is not running or has no DNS name" >&2; exit 1; }
base="https://$host$SERVE_PATH"

xcodegen generate --spec "$root/apple/project.yml" --project "$root/apple" || exit 1
xcodebuild -project "$root/apple/Wheel.xcodeproj" -scheme WheelPhone -configuration Debug \
  -destination 'generic/platform=iOS' -derivedDataPath "$DD" -allowProvisioningUpdates \
  -archivePath "$work/Wheel.xcarchive" DEVELOPMENT_TEAM="$IOS_DEVELOPMENT_TEAM" \
  CURRENT_PROJECT_VERSION="$stamp" archive || exit 1

plutil -create xml1 "$work/export.plist"
plutil -insert method -string "$METHOD" "$work/export.plist"
plutil -insert teamID -string "$IOS_DEVELOPMENT_TEAM" "$work/export.plist"
plutil -insert signingStyle -string automatic "$work/export.plist"
xcodebuild -exportArchive -archivePath "$work/Wheel.xcarchive" -exportPath "$work/export" \
  -exportOptionsPlist "$work/export.plist" -allowProvisioningUpdates || exit 1
ipa="$(ls "$work"/export/*.ipa | head -1)"
[ -f "$ipa" ] || { echo "the export made no .ipa in $work/export" >&2; exit 1; }

info="$work/Wheel.xcarchive/Products/Applications/Wheel.app/Info.plist"
version="$(plutil -extract CFBundleShortVersionString raw "$info")" || exit 1
build="$(plutil -extract CFBundleVersion raw "$info")" || exit 1

mkdir -p "$SERVE"
cp "$ipa" "$SERVE/Wheel.ipa"
m="$SERVE/manifest.plist"
plutil -create xml1 "$m"
plutil -insert items -array "$m"
plutil -insert items.0 -dictionary "$m"
plutil -insert items.0.assets -array "$m"
plutil -insert items.0.assets.0 -dictionary "$m"
plutil -insert items.0.assets.0.kind -string software-package "$m"
# The build number in the URL keeps iOS from installing a cached earlier package.
plutil -insert items.0.assets.0.url -string "$base/Wheel.ipa?b=$build" "$m"
plutil -insert items.0.metadata -dictionary "$m"
plutil -insert items.0.metadata.bundle-identifier -string "$BUNDLE" "$m"
plutil -insert items.0.metadata.bundle-version -string "$version" "$m"
plutil -insert items.0.metadata.kind -string software "$m"
plutil -insert items.0.metadata.title -string Wheel "$m"
plutil -lint -s "$m" || exit 1
[ "$(plutil -extract items.0.metadata.bundle-identifier raw "$m")" = "$BUNDLE" ] || { echo "manifest bundle id is wrong" >&2; exit 1; }

# The manifest URL is a query value, so it is percent-encoded; a raw `?` in it stops iOS acting on the link.
murl="$(python3 -I -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$base/manifest.plist?b=$build")"
link="itms-services://?action=download-manifest&url=$murl"
cat > "$SERVE/index.html" <<EOF
<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Wheel $version ($build)</title>
<body style="font:17px -apple-system;text-align:center;padding:40px">
<p>Wheel $version ($build)</p>
<p><a id="install" href="$link" style="font-size:22px">Install</a></p>
<pre id="out" style="text-align:left;white-space:pre-wrap;font:13px ui-monospace,monospace"></pre>
<script>
// Everything the page sees goes on screen and to the Mac's log, so a failed install says why.
var out = document.getElementById("out");
function say(line) {
  out.textContent += new Date().toLocaleTimeString() + " " + line + "\n";
  try { navigator.sendBeacon("log", line); } catch (e) {}
}
window.onerror = function (m, src, l) { say("error: " + m + " at " + src + ":" + l); };
say("page loaded; ua=" + navigator.userAgent);
if (!/Safari\//.test(navigator.userAgent) || /CriOS|FxiOS|EdgiOS/.test(navigator.userAgent)) {
  say("this is not Safari: only Safari opens the iOS installer. Open this page in Safari.");
}
// Fetches made before the tap (the deploy script checks the manifest itself) are the baseline.
var base = { manifest: 0, package: 0 };
fetch("status", { cache: "no-store" }).then(function (r) { return r.json(); }).then(function (s) {
  base = { manifest: s.manifest.length, package: s.package.length };
});
document.getElementById("install").addEventListener("click", function () {
  say("tapped Install");
  var start = Date.now(), had = base, warned = false;
  var poll = setInterval(function () {
    fetch("status", { cache: "no-store" }).then(function (r) { return r.json(); }).then(function (s) {
      if (s.manifest.length > had.manifest) say("iOS fetched the manifest; an install prompt should show");
      if (s.package.length > had.package) say("iOS downloaded the app; it installs on the Home Screen");
      had = { manifest: s.manifest.length, package: s.package.length };
      if (!warned && had.manifest === base.manifest && Date.now() - start > 6000) {
        warned = true;
        say("iOS has not fetched the manifest 6s after the tap: it ignored the link. Check this is Safari itself, not an in-app browser or a Home Screen web app.");
      }
      if (had.package > 0 || Date.now() - start > 120000) clearInterval(poll);
    }).catch(function (e) { say("status check failed: " + e); clearInterval(poll); });
  }, 1000);
});
</script>
EOF

# The Mac App Store Tailscale cannot serve a folder, so it proxies to a local server instead.
python3 -I "$here/phone-ota-serve.py" "$SERVE" "$PORT" "$WAIT" 2>"$work/access.log" &
server=$!
tailscale serve --bg --set-path "$SERVE_PATH" "http://127.0.0.1:$PORT" >/dev/null || exit 1
# The first request after `tailscale serve` waits on its certificate, so the limit is generous.
code="$(curl -s -m 120 -o /dev/null -w '%{http_code}' --retry 5 --retry-connrefused --retry-delay 1 "$base/manifest.plist")"
[ "$code" = 200 ] || { echo "tailscale serve answers $code for $base/manifest.plist" >&2; exit 1; }
echo "Wheel $version ($build) is served. On the phone, open in Safari: $base/"
echo "waiting up to ${WAIT}s for the phone to download it"
wait "$server"; rc=$?
server=""
tailscale serve --set-path "$SERVE_PATH" off >/dev/null 2>&1
[ "$rc" = 0 ] || { echo "no download within ${WAIT}s; the page is down" >&2; exit 1; }
echo "the phone downloaded the package; the page is down. Open Wheel on the phone once it installs."
