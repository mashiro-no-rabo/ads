---
name: land
description: >-
  Land changes in the ads repository as one commit on master using jj and
  jj up. Invoke only when the user explicitly requests landing, not for
  review, preparation, passing checks, or skill installation.
disable-model-invocation: true
metadata:
  delta-action: land
---

# Land changes

Carry out the requested landing through publication and remote verification.
The invocation supplies permission to land; do not ask for that permission
again. Stop for genuine blockers or ambiguous scope, not routine confirmation.

## Prepare

1. Read applicable project instructions. Inspect `jj status`, the requested
   diff, and the commit graph. Identify all requested changes, including
   uncommitted work, and preserve unrelated work. If scope is unclear, ask.
   Do not include unrelated commits or files.
2. Verify that `origin` is the intended publication remote for
   `mashiro-no-rabo/ads`, never the `local` checkout backlink. Check current
   contribution requirements and GitHub rules for `master`, including reviews,
   signing, and required checks. They were absent at setup; do not assume they
   remain absent. Stop if a new requirement cannot be satisfied by this
   direct-push workflow; do not bypass it.
3. Inspect `jj config get aliases.up` and relevant push configuration.
   The existing alias moves the single eligible ancestor bookmark to `@-`,
   excluding bookmarks selected by `NO_MOVE`, then runs `jj git push`.
   Confirm the eligible bookmark will be `master`, the push destination will
   be `origin`, and no unrelated references will be published. Stop if not.
   Do not pass `--help` to the alias: it executes its shell body.

## Rebase and verify

1. Fetch the publication remote with `jj git fetch --remote origin`.
   Rebase the complete requested change onto the latest `master@origin`
   before committing or running final checks. For a single working-copy
   change, use `jj rebase -r @ -d master@origin`; for a local change stack,
   select and rebase the requested stack, not unrelated changes.
   Never rewrite commits already published on `master`.
2. Resolve straightforward conflicts automatically when the intended result
   is clear. Preserve both the requested behavior and unrelated upstream
   changes. For ambiguous conflicts, report the conflict and stop for the
   user's decision. Never push unresolved conflicts.
3. Consolidate the requested changes into one unpublished change directly
   above `master@origin`, using non-interactive `jj squash` if necessary.
   Review the resulting diff. Keep applicable tests, documentation, and
   dependency lockfile updates with the change.
4. Run `just test` and `just lint`. Source: `justfile` defines `test` as
   `cargo test`, and `lint` as `cargo clippy --all-targets -- -D warnings`
   followed by `cargo fmt --check`. Use a Rust toolchain supporting the
   edition and dependencies declared in `Cargo.toml`.
   If `just` is unavailable, run those exact Cargo commands directly.
   All must pass on the final tree; after fixes or another rebase, rerun
   affected verification. Do not install tools without permission.
5. Verify all applicable required remote checks and reviews have passed for
   the exact changes to be landed. Pending, failing, missing, or unverifiable
   required checks are blockers. Local success does not replace required
   remote checks. If no remote checks are required, do not invent any.

## Commit, publish, and confirm

1. Create one commit with a short, plain, imperative message using
   `jj commit -m "<message>"`. Avoid an interactive editor. If the requested
   change is already one complete commit with a suitable message and an
   empty working-copy change above it, reuse it rather than creating an
   empty commit. Verify that `@-` is the one requested commit directly above
   the fetched `master@origin`, that the working copy is empty, and that all
   checks apply to its tree.
2. Check that `master` can move forward to this commit and that the
   inspected `jj up` alias will select only `master`. If its local bookmark
   is behind the fetched remote, advance it to `master@origin` with
   `jj bookmark move master --to master@origin` only after confirming this
   does not discard unrelated local work.
3. Run `jj up` to publish. Source: the user's `aliases.up` configuration,
   inspected above, implements the bookmark move and `jj git push`.
   Reinspect that definition at execution time; do not replace it with a
   different publication workflow or use the `local` remote.
4. If the remote has advanced, fetch again, rebase onto its latest `master`,
   resolve conflicts under the policy above, and rerun verification before
   retrying. Never force-push or rewrite shared history.
5. Verify the actual `refs/heads/master` on `origin`, using
   `git ls-remote origin refs/heads/master`, matches the landed commit's
   full Git commit ID from `jj log -r @- --no-graph -T commit_id`.
   Also confirm the local `master` bookmark points there and `jj status`
   shows no unexpected changes.
   A successful commit, moved bookmark, or started push alone is not landing
   success. Report the commit and destination only after remote verification.
   If blocked, explicitly report that the changes have not landed and why.
