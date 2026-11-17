# Conventions

How changes reach `main`: linear history, pull requests and branch protection. Also how
code comments refer to UI controls.
Back to the [README](../README.md).

`main` is **linear** — no merge commits — and every commit on it is an **atomic unit**: it
builds and passes the gate on its own. Work happens on a feature branch, which lands as
**one squashed commit**.

```sh
scripts/check-all.sh                # the local mirror of CI
```

Pull requests are the only route to `main` — there is no local landing script. Push the
branch, open a PR, and squash-merge it in the Forgejo web UI once CI is green:

```sh
git push -u origin my-branch
# open a PR at forge.daveynet.xyz, squash-merge once CI is green
```

CI runs against the PR head on every push, so its result describes the commit you are about
to merge: `rust` (fmt, clippy and tests under `ssr`), `hydrate` (the same tests recompiled
under the client feature set) and `wasm` (the release build plus the bundle-size budget).

What a web-UI merge does **not** do, the way the retired `scripts/land.sh` did, is check the
**merged tree**. `land.sh` ran the gate on the merged tree *before the commit existed* and
refused to commit on failure — that ordering is what made "every commit on `main` passes the
gate" a property rather than a hope. A squash-merge inverts it: the commit is built from the
PR head, and merging is a decision a person makes by reading CI's result, not a check the
merge itself re-runs.

**`main` is branch-protected** (enabled 2026-08-04), which is what stands in for that
ordering:

- direct pushes are off — a PR is the only way in
- `ci / rust`, `ci / hydrate` and `ci / wasm`, in their `(pull_request)` form, are required
  status checks
- `ci / docker` is deliberately **not** required. It is skipped on pull requests, and Forgejo
  reports a skipped job as `success` on the commit-status API — so requiring it would gate on
  nothing at all. The three jobs above carry no `if:` condition, which is what makes requiring
  them directly safe here.
- `advisories / cargo-deny` is not required either: `.forgejo/workflows/advisories.yml`'s header says
  how to accept a finding, including the `ADVISORIES_MODE` switch for all of them at once
- `block_on_outdated_branch` is on, so a PR green against a `main` that has since moved cannot
  merge
- `apply_to_admins` is on, so the rule binds the repository owner too — without it the whole
  rule is advisory for the only account that merges

That is not identical to gating the merged tree — the merge commit itself is still never
tested before it exists — but it removes the two ways the gap actually bites: a red head, and
a stale base.

A plain `git merge --squash` discards every commit message on the branch, so `land.sh` used to
prefill the squash message with all of them verbatim. A PR's written body does that job now,
and does it better as prose. What moves is *where* the intermediate messages live: they are no
longer inside the commit object that lands on `main`. They still exist, on the PR page and
under `refs/pull/N/head` — but only on `forge.daveynet.xyz`, a dependency the git history
alone did not previously have.

Once per clone, so a non-fast-forward merge fails rather than quietly creating a merge
commit:

```sh
git config merge.ff only && git config pull.ff only
```

That is convenience — `.git/config` is untracked and binds nobody. The remote is the real
constraint: it allows only squash and fast-forward merges, and deletes the branch on merge.

## Code comments

A comment about a UI control names its component, not the text on it: write
``[`EarlierControl`]`` (or describe what it does, such as "the lookback"), not "‹ earlier".
Labels get shortened and reworded. When the date controls became "‹ earlier" and
"later ›", 25 comment lines across nine files quoted the old labels and had to change
too. A component name changes less often, and in a `///` doc comment it is a link that
`check-all.sh`'s rustdoc step fails on once it dangles.

Text written for users has to quote the label, so a label change still reaches the About
page (`src/app/pages/about.rs`) and `config.toml.example`.
