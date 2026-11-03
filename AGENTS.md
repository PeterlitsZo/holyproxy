# AGENTS.md

## Private Deployment Configuration

- Never put real account IDs, resource IDs, credentials, or production data in
  tracked files, examples, documentation, commit messages, or user-facing output.
  Redact private values before displaying tool output.
- Keep `wrangler.jsonc` generic. Store deployment-specific configuration only in
  the gitignored `wrangler.private.jsonc`; keep local exports under `.wrangler/`.
- Never let Wrangler auto-provisioning or `--update-config` write real resource
  IDs into tracked configuration. Use an ignored private configuration instead.
- Before staging or committing, inspect both the working tree and staged changes
  for private configuration. Verify private files are ignored and not tracked.

## Behavior

- DO NOT add tests.

## When You Need to Commit

WHEN THE USER ASKS YOU TO COMMIT, you MUST follow this section.

### Before Committing

Before committing, you MUST:

- Explicitly decide whether `CHANGELOG.md` needs an update. Keep it concise.
  Add only user-visible changes such as new features, behavioral changes,
  compatibility-impacting fixes, or important bug fixes. Do not add pure
  internal refactors that do not affect functionality. If you update
  `CHANGELOG.md`, you MUST ask the user to confirm that it looks correct before
  committing, and you MUST stage it explicitly.

### Git Commit Message

Git commit messages use the following format:

```
<type>: <subject>
```

Here, `<type>` describes the nature of the change, such as `feat`, `fix`,
`docs`, `chore`, or `refactor`.

The `<subject>` should be a short description of the change in English. Keep it
concise and clear. You need to review all changed files and summarize the most
important change (for example, if the project includes both a refactor and a
documentation update, the subject should describe the refactor rather than the
documentation update). The subject MUST end with an English full stop, and its
first letter MUST be capitalized.
