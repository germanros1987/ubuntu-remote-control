# Contributing / branch & merge policy

## Workflow

1. Do all work on a `task/<id>` (or `fix/<name>` / `feature/<name>`) branch cut from
   an up-to-date `main`.
2. Push that branch to `origin` (`git push -u origin <branch>`).
3. Open a pull request against `main` and wait for a reviewer/maintainer to
   approve and merge it.

`main` only moves via a merged, reviewed pull request. This applies to every
change, regardless of size or urgency.

## If you can't push or open a PR

Some worker GitHub identities are pull-only on `origin` (`permissions.push:
false`), which blocks step 2/3 above. When that happens:

- **Do not** merge your branch into `main` locally and push `main` directly,
  even with `--no-ff` and even if you leave a clear commit trail. A local
  merge pushed straight to `origin/main` has no PR, no reviewer, and no
  approval — it silently bypasses the review gate this workflow exists to
  enforce, no matter how well-documented the commit message is.
- This ban is on the destination and the credential, not on one particular
  transport. It applies just as much to pushing over SSH using an ambient
  or agent-forwarded key that happens to have write access to `origin` —
  because it's a human's key, a leftover agent socket, or another
  identity's key already loaded on a shared machine — as it does to pushing
  over HTTPS with a personal access token. **Do not** push to `main` (or
  merge into it) as an identity other than the one assigned to you for this
  task, regardless of how that identity's access reaches you. Being pull-only
  under your own identity is not a problem to solve by pushing as someone
  else's; it's the blocker described below.
- **Do not** silently drop the work either.
- Push what you can (a fork, if you have write access there, works — see
  cross-fork PRs below), leave the branch exactly as it is otherwise, and
  **file a blocker** describing the access problem. Stop there. A blocked
  task waiting on human action is the correct outcome, not a failure to
  route around.

## Cross-fork PRs (workaround for pull-only accounts)

If your account can push to a fork of this repo but not to `origin` itself,
you can still open a review-visible PR:

```
git push fork <branch>
gh pr create --repo germanros1987/ubuntu-remote-control \
  --head <your-account>:<branch> --base main
```

GitHub allows a PR from a fork branch into the upstream repo without push
access to the upstream repo. The PR still needs a maintainer with write
access on `origin` to review and merge it — you cannot merge it yourself.
This is the accepted fallback; merging directly to `main` is not.

## Why this matters

A merge that lands in `main` without a PR is indistinguishable, after the
fact, from a properly reviewed one unless someone reads the commit body
closely. That defeats the purpose of requiring review in the first place.
If the push/PR path is broken, that is a process bug to report and wait
on — not something to engineer around by merging locally.
