# Contributing

## Development

The plugin is a Rust cdylib wrapping the ImageGlass SDK v1.1.0 codec contract. All decode logic lives in the upstream [`ithmb-core`](https://github.com/B67687/Ithmb-Codec) crate — this repo is the C ABI glue. Changes to decoding behavior belong upstream.

```bash
# Check (fast, recommended during development)
cargo check

# Build
cargo build --release

# Lints (CI enforces clippy all + pedantic as deny)
cargo clippy --all-features --all-targets -- -D warnings

# Format
cargo fmt --check

# Tests
cargo nextest run --all-features --all-targets
```

## Versioning

- The crate version (`Cargo.toml`), the plugin manifest (`igplugin.json`), and the packaging script (`scripts/package.sh`) must stay in lockstep. Bump them together.
- The toolchain is pinned by `rust-toolchain.toml`; `Cargo.lock` is committed — keep both consistent with `Cargo.toml` so builds are reproducible.
- Releases are created by pushing a `v*` tag — CI builds all 3 platforms and publishes a GitHub Release with the packaged `.igplugin.zip` artifacts.

## Committing

- **Atomic commits** — one logical change per commit; split unrelated changes into separate commits.
- **Present-tense imperative subjects** (e.g. "Add …", "Fix …", "Drop …").
- **Signed commits** — every commit must be signed (`git commit -S`); unsigned commits are rejected.
- No `Co-authored-by` or tool-attribution trailers — the commit author field is the sole attribution.

## CI

GitHub Actions runs build + clippy + deny + packaging on every push and PR. All third-party actions are SHA-pinned in `.github/workflows/ci.yml`; do not introduce an unpinned action.

## Security

See `SECURITY.md` for the security policy and the private reporting channel.
