#!/usr/bin/env bash
# Download, VERIFY, then install an RGB Forge release. Nothing is unpacked or run unless
# the download passes both checks:
#
#   1. Its SHA-256 equals the one GitHub computed when the file was uploaded (from the
#      GitHub API, not from a file that travels with the download).
#   2. Its signed build provenance proves it was built by this repository's release
#      workflow from a v* tag (needs the GitHub CLI, logged in, or cosign).
#
# Usage:  bash get.sh [v0.2.0]        (default: the latest release)
#         bash get.sh --checksum-only [tag]   skip check 2 (not recommended)
set -euo pipefail

REPO=Al-Hadrusi/rgbforge
WORKFLOW=.github/workflows/release.yml
API=https://api.github.com/repos/$REPO

checksum_only=0
if [ "${1:-}" = "--checksum-only" ]; then checksum_only=1; shift; fi
TAG="${1:-}"

die() { echo "STOPPED: $*" >&2; exit 1; }
command -v curl >/dev/null || die "curl is needed."
command -v sha256sum >/dev/null || die "sha256sum is needed."

if [ -z "$TAG" ]; then
    TAG=$(curl -fsSL "$API/releases/latest" | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
fi
case "$TAG" in v[0-9]*) ;; *) die "Couldn't find a release (got '$TAG')." ;; esac
case "$TAG" in *[!0-9A-Za-z.+v-]*) die "Unexpected characters in tag '$TAG'." ;; esac
NAME="rgbforge-${TAG#v}-x86_64-linux"
FILE="$NAME.tar.gz"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

echo "==> Downloading $FILE ($TAG)"
curl -fsSL -o "$FILE" "https://github.com/$REPO/releases/download/$TAG/$FILE"

echo "==> Check 1: SHA-256 against GitHub's own record"
# Split the API's JSON into lines and pair each asset's "name" with its "digest".
expected=$(curl -fsSL "$API/releases/tags/$TAG" | tr ',{}' '\n' |
    sed -n 's/^ *"\(name\|digest\)": *"\([^"]*\)"$/\1 \2/p' |
    awk -v f="$FILE" '$1=="name"{n=$2} $1=="digest" && n==f {print $2; exit}')
actual="sha256:$(sha256sum "$FILE" | cut -d' ' -f1)"
[ -n "$expected" ] || die "GitHub has no checksum on record for $FILE."
[ "$expected" = "$actual" ] || die "Checksum mismatch: expected $expected, got $actual. The download was altered or corrupted."
echo "    OK ($actual)"

if [ "$checksum_only" -eq 1 ]; then
    echo "==> Check 2 skipped (--checksum-only): the build's origin was NOT verified."
else
    echo "==> Check 2: signed build provenance"
    verified=0
    if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
        gh attestation verify "$FILE" --repo "$REPO" --signer-workflow "$REPO/$WORKFLOW" \
            --source-ref "refs/tags/$TAG" && verified=1
    fi
    if [ "$verified" -eq 0 ] && command -v cosign >/dev/null; then
        if curl -fsSL -o "$FILE.sigstore.json" "https://github.com/$REPO/releases/download/$TAG/$FILE.sigstore.json"; then
            cosign verify-blob-attestation --new-bundle-format --bundle "$FILE.sigstore.json" \
                --type https://slsa.dev/provenance/v1 \
                --certificate-oidc-issuer https://token.actions.githubusercontent.com \
                --certificate-identity "https://github.com/$REPO/$WORKFLOW@refs/tags/$TAG" \
                "$FILE" && verified=1
        fi
    fi
    if [ "$verified" -eq 0 ]; then
        die "Couldn't verify who built this download. Install the GitHub CLI and run 'gh auth login'
         (Arch: sudo pacman -S github-cli), or install cosign (Arch: sudo pacman -S cosign), then try
         again. Releases older than the signing workflow have no signature; for those, run with
         --checksum-only if you accept that."
    fi
    echo "    OK: built by $REPO's release workflow from $TAG"
fi

echo "==> Unpacking"
# Every entry must stay inside the release folder: no absolute paths, no '..'.
if tar -tzf "$FILE" | grep -qvE "^$NAME(/[A-Za-z0-9._+-]+)*/?$"; then
    die "The archive contains unexpected paths."
fi
tar -xzf "$FILE" --no-same-owner --no-same-permissions
cd "$NAME"
./install.sh
