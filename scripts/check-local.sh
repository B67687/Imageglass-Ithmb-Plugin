#!/usr/bin/env bash
# check-local.sh — the full local CI for the ImageGlass Ithmb plugin.
#
# One command runs every local-movable gate the GitHub CI runs, locally:
#
#   1. cargo clippy   (matches CI verify_clippy; Cargo.toml governs levels)
#   2. cargo nextest  (matches CI verify_clippy)
#   3. cargo build --release + symbol export verify + ABI smoke (matches CI build, Linux)
#   4. cargo-deny     (matches CI verify_deny, pinned musl binary)
#   5. gitleaks       (matches CI secrets, all commits)
#   6. F-### anchor  (every test covers ≥1 feature)
#   7. wc -l fitness  (source files ≤250 LOC non-test)
#
# Checks that MUST stay remote (hardware ceiling / release infra) are NOT here:
#   - 3-OS matrix builds (macOS / Windows) + Windows PE export table verify
#   - package.sh artifact assembly (dist/*.igplugin.zip)
#   - GitHub Release creation (tags)
#
# Exit 0 = everything green. Non-zero = a gate failed.
set -e
cd "$(dirname "$0")/.."

echo "── check:local — full local CI ──"

echo "── [1] cargo clippy (all-features, all-targets; Cargo.toml governs)"
cargo clippy --all-features --all-targets

echo "── [2] cargo nextest (all-features, all-targets)"
cargo nextest run --all-features --all-targets

echo "── [3] cargo build --release + symbol export + ABI smoke (Linux)"
cargo build --release
LIB=""
for cand in target/release/libithmb_core_cabi.so target/release/libithmb_core_cabi.dylib; do
  if [ -e "$cand" ]; then LIB="$cand"; break; fi
done
if [ -n "$LIB" ]; then
  if nm -D "$LIB" 2>/dev/null | grep -q ig_plugin_get_api \
     || nm -gU "$LIB" 2>/dev/null | grep -q ig_plugin_get_api; then
    echo "Symbol verified"
  else
    echo "Missing symbol!"
    exit 1
  fi
else
  echo "No cdylib artifact found"
  exit 1
fi
python3 scripts/abi-smoke.py tests/fixtures/test1.ithmb

echo "── [4] cargo-deny (pinned 0.20.2 musl, matches CI)"
DENY_BIN="${CARGO_DENY_BIN:-/tmp/cargo-deny}"
if [ ! -x "$DENY_BIN" ]; then
  curl -sSfL https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz -o /tmp/cargo-deny.tar.gz
  tar -xzf /tmp/cargo-deny.tar.gz -C /tmp --strip-components=1
  DENY_BIN=/tmp/cargo-deny
fi
"$DENY_BIN" --log-level warn --manifest-path ./Cargo.toml --all-features check

echo "── [5b] cargo machete (unused deps; CI verify_machete is the hard gate)"
if cargo machete --help >/dev/null 2>&1; then
  cargo machete
else
  echo "(skip: cargo-machete not installed locally; run: cargo install cargo-machete --version 0.9.2 --locked)"
fi

# Version SST: igplugin.json version must match Cargo.toml (single source of truth)
echo "── [5] version SST (igplugin.json ↔ Cargo.toml)"
CARGO_VER=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
IGPJ_VER=$(grep '"version"' igplugin.json | sed 's/.*"version".*:.*"\([^"]*\)".*/\1/')
if [ "$CARGO_VER" != "$IGPJ_VER" ]; then
  echo "Version mismatch: Cargo.toml=$CARGO_VER igplugin.json=$IGPJ_VER"
  echo "Update igplugin.json or use package.sh (generates manifest from Cargo.toml)"
  exit 1
fi
echo "Version OK: $CARGO_VER"


# F-### anchor grep: every #[test] fn must have a F-### trace tag in the 5 lines above
echo "── [7] F-### anchor grep (test coverage traceability)"
MISSING_ANCHORS=()
while IFS= read -r match; do
  # match is like: src/file.rs:243:    #[test]
  file=$(echo "$match" | cut -d: -f1)
  test_lineno=$(echo "$match" | cut -d: -f2)
  fn_lineno=$((test_lineno + 1))
  fn_name=$(sed -n "${fn_lineno}p" "$file" | grep -oP 'fn \K[a-z_]+')
  if [ -z "$fn_name" ]; then continue; fi
  # Check the 5 lines before #[test] for F-### tag
  start=$((test_lineno > 5 ? test_lineno - 5 : 1))
  anchor=$(sed -n "${start},${test_lineno}p" "$file" | grep -c 'F-[0-9]\{3\}' || true)
  if [ "$anchor" -eq 0 ]; then
    MISSING_ANCHORS+=("$fn_name ($file:$fn_lineno)")
  fi
done < <(grep -rn '#\[test\]' src/*.rs)
if [ ${#MISSING_ANCHORS[@]} -gt 0 ]; then
  echo "Missing F-### trace tags in tests: ${MISSING_ANCHORS[*]}"
  exit 1
fi
echo "All tests have F-### trace tags"

# wc -l fitness: non-test source files ≤250 LOC
echo "── [8] wc -l fitness (source files ≤250 LOC)"
OVER_THRESHOLD=()
for f in src/*.rs; do
  # Skip test modules (rough heuristic: files with only tests)
  total=$(wc -l < "$f")
  # Count non-test lines (exclude #[cfg(test)] mod tests and everything after)
  non_test=$(sed '/^#\[cfg(test)\]/,$d' "$f" | wc -l)
  if [ "$non_test" -gt 250 ]; then
    OVER_THRESHOLD+=("$f ($non_test LOC)")
  fi
done
if [ ${#OVER_THRESHOLD[@]} -gt 0 ]; then
  echo "WARNING: Files exceeding 250 LOC threshold: ${OVER_THRESHOLD[*]}"
  echo "( tracked in TECH_DEBT_AUDIT.md A1/A2 — not blocking )"
fi
echo "wc -l fitness check complete"
GITLEAKS_BIN="${GITLEAKS_BIN:-/tmp/gitleaks}"
if [ ! -x "$GITLEAKS_BIN" ]; then
  curl -sSfL "https://github.com/gitleaks/gitleaks/releases/download/v8.24.3/gitleaks_8.24.3_linux_x64.tar.gz" -o /tmp/gitleaks.tar.gz
  tar -xzf /tmp/gitleaks.tar.gz -C /tmp gitleaks
  GITLEAKS_BIN=/tmp/gitleaks
fi
"$GITLEAKS_BIN" git --no-banner --log-opts="--no-merges --all"

echo
echo "── check:local — ALL GREEN ──"
