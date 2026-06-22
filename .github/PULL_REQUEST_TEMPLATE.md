<!--
Thanks for contributing! Keep the title a valid Conventional Commit
(feat|fix|perf|refactor|docs|test|build|ci|chore|revert), e.g.
`feat(syntax): add lambda parsing`. The squash-merge commit uses the PR title.
-->

## Summary

<!-- What does this PR change, and why? -->

## Checklist

- [ ] Tests added or updated (or N/A, explained below).
- [ ] `cargo xtask ci` passes locally (fmt, clippy `-D warnings`, test, wasm-check, deny).
- [ ] A changeset was added (`.changeset/*.md`) if this changes the npm-facing surface (`@gdscript-analyzer/*`).
- [ ] Docs updated (mdBook under `docs/`) and/or a new ADR added under `docs/adr/` if the change is architecturally consequential.
- [ ] PR title is a valid Conventional Commit (see comment above).

## Notes

<!-- Anything reviewers should know: tradeoffs, follow-ups, related issues. -->
