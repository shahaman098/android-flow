#!/usr/bin/env bash
# Install the built bundle to a stable path so macOS Accessibility stays granted.
#
# macOS keys TCC (Accessibility) approval on bundle path + code signature. Running
# the bundle straight out of target/ with the linker's ad-hoc signature meant the
# approval never stuck: AXIsProcessTrusted stayed false and the § event tap never
# installed. Signing with a real identity gives a stable designated requirement,
# so one approval survives every rebuild.

set -euo pipefail

BUILT="src-tauri/target/release/bundle/macos/Flow.app"
DEST="/Applications/Flow.app"
BUNDLE_ID="com.efi.voiceflow"
ENTITLEMENTS="src-tauri/entitlements.plist"
SIGNING_IDENTITY="$(jq -r '.bundle.macOS.signingIdentity // empty' src-tauri/tauri.conf.json)"

if [[ ! -d "$BUILT" ]]; then
  echo "error: $BUILT not found — run 'pnpm tauri build' first." >&2
  exit 1
fi

# DMG packaging can replace the executable after Tauri's first signing pass.
# Re-sign the final on-disk bundle so the copy installed below always has the
# stable designated requirement macOS uses for Accessibility approval.
if [[ -z "$SIGNING_IDENTITY" ]]; then
  echo "error: bundle.macOS.signingIdentity is missing from tauri.conf.json." >&2
  exit 1
fi
if ! security find-identity -v -p codesigning | grep -F "$SIGNING_IDENTITY" >/dev/null; then
  echo "error: signing identity not available in Keychain: $SIGNING_IDENTITY" >&2
  exit 1
fi

codesign --force --options runtime --entitlements "$ENTITLEMENTS" \
  --sign "$SIGNING_IDENTITY" "$BUILT/Contents/MacOS/flow-app"
codesign --force --options runtime --entitlements "$ENTITLEMENTS" \
  --sign "$SIGNING_IDENTITY" "$BUILT"
codesign --verify --deep --strict --verbose=1 "$BUILT"

# A previous build linked Swift concurrency through @rpath without emitting any
# LC_RPATH, so it installed successfully and crashed in dyld on every launch.
if otool -L "$BUILT/Contents/MacOS/flow-app" | grep -q '@rpath/libswift' \
  && ! otool -l "$BUILT/Contents/MacOS/flow-app" | grep -q 'LC_RPATH'; then
  echo "error: Flow references @rpath Swift libraries but has no LC_RPATH." >&2
  exit 1
fi

if ! codesign -dvvv "$BUILT" 2>&1 | grep -F "Authority=Apple Development:" >/dev/null; then
  echo "error: $BUILT does not have an Apple Development signature." >&2
  exit 1
fi

# Only ever replace our own bundle at $DEST.
if [[ -e "$DEST" ]]; then
  existing="$(defaults read "$DEST/Contents/Info" CFBundleIdentifier 2>/dev/null || echo "")"
  if [[ "$existing" != "$BUNDLE_ID" ]]; then
    echo "error: $DEST exists but is not $BUNDLE_ID (found '${existing:-unknown}')." >&2
    echo "       Refusing to replace it. Move it aside manually." >&2
    exit 1
  fi
  pkill -f "$DEST/Contents/MacOS/" 2>/dev/null || true
  sleep 1
  chflags -R nouchg "$DEST" 2>/dev/null || true
  rm -rf "$DEST"
fi

ditto "$BUILT" "$DEST"
codesign --verify --deep --strict --verbose=1 "$DEST"

# Verify that dyld can load the executable before reporting a successful install.
# Launch it directly, wait briefly, and require it to still be alive.
"$DEST/Contents/MacOS/flow-app" >/tmp/flow-install-smoke.log 2>&1 &
smoke_pid=$!
sleep 2
if ! kill -0 "$smoke_pid" 2>/dev/null; then
  wait "$smoke_pid" || true
  echo "error: installed Flow exited during launch smoke test." >&2
  cat /tmp/flow-install-smoke.log >&2 || true
  exit 1
fi
kill "$smoke_pid" 2>/dev/null || true
wait "$smoke_pid" 2>/dev/null || true

echo "installed → $DEST"
codesign -dvvv "$DEST" 2>&1 | grep -E "^(Identifier|Signature|Authority=Apple Development|Sealed Resources|TeamIdentifier)" || true
echo
echo "Grant Accessibility to $DEST once:"
echo "  System Settings → Privacy & Security → Accessibility → + → /Applications/Flow.app"
echo "Remove any older Flow entries there first — stale rows block the new signature."

open "$DEST"
