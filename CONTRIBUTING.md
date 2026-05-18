# Contributing

## Commit Style

Use [Conventional Commits](https://www.conventionalcommits.org/) for all commit
messages.

Format:

```text
<type>[optional scope]: <description>
```

Common types:

- `feat`: user-facing or project capability changes
- `fix`: bug fixes
- `docs`: documentation-only changes
- `test`: test additions or corrections
- `refactor`: code changes that do not alter behavior
- `perf`: performance improvements
- `chore`: maintenance changes
- `ci`: CI configuration changes

Examples:

```text
feat: initialize decision integrity baseline
docs: clarify admission controller goals
test: cover replay hash stability
perf: avoid hot-path packet allocation
```

Use an imperative, lowercase description and avoid trailing punctuation. For
breaking changes, add `!` after the type or scope and describe the impact in the
commit body.
