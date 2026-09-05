#!/usr/bin/env bash
#
# Builds the three binaries and assembles them.
#
# Unsigned by default, on purpose. Signing needs certificates that are the
# Operator's and are not in this repository, so a contributor running this
# gets a working unsigned build rather than an error about a missing
# identity — and each skipped step says so, instead of quietly producing an
# artifact that looks signed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/packaging/out"
rm -rf "$out"
mkdir -p "$out"

echo "== core and sidecar =="
cargo build --release --manifest-path "$root/Cargo.toml" \
  -p evertranscript -p evertranscript-summarizer

echo "== client =="
pnpm -C "$root/clients/electron" install --frozen-lockfile
pnpm -C "$root/clients/electron" build

for binary in evertranscript evertranscript-summarizer; do
  suffix=""
  [[ "${OS:-}" == "Windows_NT" ]] && suffix=".exe"
  cp "$root/target/release/${binary}${suffix}" "$out/"
done
cp -R "$root/clients/electron/dist" "$out/client"

echo
echo "== checksums =="
# Written beside the artifacts so the cask and manifest are generated from
# them rather than hand-edited. A hand-maintained hash is one that will
# eventually be wrong.
(
  cd "$out"
  if command -v sha256sum >/dev/null 2>&1; then
    find . -type f ! -name SHA256SUMS -exec sha256sum {} + | tr -d '\\' > SHA256SUMS
  else
    find . -type f ! -name SHA256SUMS -exec shasum -a 256 {} + > SHA256SUMS
  fi
)
echo "artifacts and SHA256SUMS in $out"

echo
if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
  echo "== signing =="
  for binary in evertranscript evertranscript-summarizer; do
    # Both identifiers must agree, or macOS refuses to launch the child and it
    # presents as the Core "not starting", with nothing in any log to say why.
    codesign --force --timestamp --options runtime \
      --entitlements "$root/packaging/macos/entitlements.plist" \
      --sign "$MACOS_SIGNING_IDENTITY" "$out/$binary"
  done
  codesign --verify --deep --strict --verbose=2 "$out/evertranscript"
  echo "signed"
else
  echo "== signing skipped: MACOS_SIGNING_IDENTITY is not set =="
  echo "   Expected without the Operator's Developer ID certificate."
  echo "   See packaging/README.md for what only they can do."
fi

echo
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  echo "== notarizing =="
  ditto -c -k --keepParent "$out" "$out/EverTranscript.zip"
  xcrun notarytool submit "$out/EverTranscript.zip" \
    --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$out/EverTranscript.zip" || true
  echo "notarized"
else
  echo "== notarization skipped: NOTARY_PROFILE is not set =="
fi
