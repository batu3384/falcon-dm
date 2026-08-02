<!-- ponytail: PR checklist — helps reviewers focus on what matters. -->

## Summary

<!-- What does this PR change, and why? One or two paragraphs. -->

## Change type

- [ ] Bug fix (non-breaking)
- [ ] New feature (non-breaking)
- [ ] Refactor / cleanup (no behavior change)
- [ ] Performance improvement
- [ ] Security fix
- [ ] Breaking change (please describe impact below)
- [ ] Docs / CI / tooling

## Checklist

- [ ] `npm run lint` passes (0 warnings)
- [ ] `npm run format:check` passes
- [ ] `npm run test` passes
- [ ] `npm run build` passes
- [ ] `cargo fmt --check` passes (in `src-tauri`)
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Tests added for new behavior
- [ ] `// ponytail:` comments added for non-obvious decisions

## Breaking changes / migration notes

<!-- If breaking: what breaks, how users/maintainers adapt. Delete if N/A. -->

## Screenshots / before-after (if UI)

<!-- Drag in images for visual changes. -->
