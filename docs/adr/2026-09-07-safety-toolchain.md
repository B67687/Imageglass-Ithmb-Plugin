# ADR-0002: SAFETY Review + Toolchain Modernization Round

**Status:** Accepted (2026-09-07)

## Context

108 unsafe sites across 9 plugin files (FFI boundary, buffer registry,
decode paths) lacked uniform safety documentation, in the same round as
Codec's review (see Codec ADR-0010). Separately, the toolchain lagged:
`cargo test` instead of nextest, no unused-dep gate, unset RUSTFLAGS, and
two workflow edits left orphan steps that made CI fail in 0s (unparseable
YAML).

## Decision

1. **Per-block `// SAFETY:` contracts** on all 108 sites, reviewed with code.
2. **Fix the real bugs, minimally:** F-P1 hostile-input logging UB
   (`try_from().unwrap_or(b'?')`), F-P2 explicit `unsafe fn`, F-P3
   const→static, E0133 inner-unsafe-block restore for edition 2024.
3. **Toolchain:** nextest 0.9.143 (taiki-e) in CI + local docs, machete
   unused-dep gate, explicit top-level `RUSTFLAGS: ""` (same injection
   lesson as Codec ADR-0009).
4. **Workflow-edit discipline:** validate the YAML parses after every
   workflow edit (a bare `- name:` kills the whole file in 0s).

## Consequences

- Unsafe surface is documented; UB fixes are in CHANGELOG (Fixed).
- `cargo test` references across RULES/AGENTS/SPEC/EXPLAINER/CONTRIBUTING
  updated to nextest; no other external test dependencies added.
