# AGENTS.md — Branch & Release Discipline (ia-gitflow)

This file is the authoritative git workflow for Baedeker. It formalizes the Industrial
Algebra gitflow so that humans and agents follow the same flow every time. The rules
below exist because each shortcut listed here has caused real damage in IA repos —
read the **Why** before considering an exception. There are no exceptions.

## Branch model

```
            feature/* ──PR──▶ develop ──release PR──▶ release/v* ──PR──▶ main ──tag v*──▶ publish
                                ▲                                                        │
                                └────────────── backmerge (merge commit) ────────────────┘
```

- **`main`** — what shipped. Releases and release-PR merges only. Tagged `v*`. Protected.
- **`develop`** — integration branch for the *next* release. Protected.
- **`feature/*`**, **`fix/*`**, **`chore/*`**, **`docs/*`** — one PR's worth of work,
  branched from `develop`, PR'd back to `develop`.
- **`release/v*`** — cut from `develop` for release-only commits (changelog date, final
  polish). PR'd to `main`.

## Hard rules

### Rule 1 — Never push directly to `main` or `develop`

Protected branches receive changes **only via merged PRs**. No `git push` to either,
ever — not "just a one-line fix", not "it's faster". Branch it, PR it, let CI run.

**Why:** Schubert v0.4.0 work was pushed straight to `develop`. CI never ran on it,
`develop` went red, and the breakage surfaced only when the next proper PR hit it —
blocking the release. The PR flow isn't ceremony; it's what runs CI before code lands.

### Rule 2 — Every merge to `main` is followed by a `main → develop` backmerge

Immediately after a release (or sync) PR merges to `main`, backmerge `main` into
`develop` using a **merge commit — never a squash**. The backmerge is the **last step
of releasing**, not an optional chore. If you tagged and published, you owe `develop`
a backmerge.

**Why:** Squash-merging a release creates a `main`-only commit that `develop`'s graph
never contains; the branches diverge instantly and invisibly until the *next* release
PR conflicts against `main`. Schubert v0.3.0's squash-with-no-backmerge caused the
v0.4.0 release-PR conflict.

### Rule 3 — Release-only commits live on a `release/*` branch

Dating the changelog and other release-specific commits go on the release branch so
they're reviewed in the release PR — not pushed to `develop` (Rule 1) or buried in a
squash. The backmerge carries them to `develop`.

### Rule 4 — `main` and `develop` must outlive every PR

Never enable GitHub's **"automatically delete head branches"** on a gitflow repo, and
never delete `main`/`develop` after merging a PR that uses one as its head (a sync or
backmerge PR does exactly this).

**Why:** Baedeker PR #46 (develop→main sync) had `develop` as its head branch; the
auto-delete setting **deleted `develop` on merge**. It was restored from a local clone
and the setting was disabled repo-wide (`delete_branch_on_merge=false`). Check this
setting on any new IA repo *before* the first release flow.

## Workflows

### Feature work

```bash
git checkout develop && git pull
git checkout -b feature/<short-scope>     # or fix/ chore/ docs/
# ... work, commit (conventional prefixes: feat: fix: chore: refactor: test: docs:) ...
git push -u origin feature/<short-scope>
gh pr create --base develop --body-file <file>   # backticks in --body get shell-mangled
```

Merge with **squash** after green CI. One work unit per PR.

### Stacked PRs

If PR B builds on unmerged PR A, branch B from A's tip. After A squash-merges:

```bash
git rebase --onto develop <A's-last-commit> B
git push --force-with-lease
```

### Rebasing a PR onto updated `develop`

Rebase and force-push your own branch (`--force-with-lease`). Never rebase shared
branches.

### Releasing

1. **Version bump** on a branch off `develop` (workspace version, internal dep
   versions, new CHANGELOG section) → PR to `develop`.
2. **Cut the release branch:** `git checkout -b release/v<ver> origin/develop`.
3. **Date the changelog** on the release branch; verify the full matrix.
4. **Open the release PR** `release/v<ver> → main`. If it conflicts against `main`,
   a previous backmerge was skipped — diagnose (below), resolve, verify.
5. **Merge** to `main` (squash is conventional).
6. **Tag** `v<ver>` on the merge commit; push the tag (publishes via CI when wired).
7. **Backmerge** `main → develop` with a **merge commit** (Rule 2). For an identical
   tree this PR shows 0 file changes — that is expected; it moves history, not content.
8. Verify: `git log --oneline origin/develop..origin/main` is empty.

### Diagnosing a conflicting release PR

```bash
git merge-base --is-ancestor origin/main origin/develop \
  && echo "clean" || echo "DIVERGED — a backmerge was skipped"
git log --oneline origin/develop..origin/main   # what main has that develop lacks
git diff --stat origin/main origin/develop      # expect metadata files only
```

`develop`'s tree is almost always a strict superset of `main`'s. Resolve conflicted
metadata (CHANGELOG, Cargo.toml, Cargo.lock) to the release branch's content,
regenerate `Cargo.lock`, run the full verification matrix, then proceed. Confirm the
superset with `git diff` per file — don't assume.

## Common pitfalls

| Shortcut | Symptom | Fix |
|---|---|---|
| Push straight to `develop` | `develop` red; CI never ran | Rule 1 — branch + PR, always |
| Squash-merge release, no backmerge | Next release PR conflicts vs `main` | Rule 2 — backmerge (merge commit) every time |
| Backmerge as a squash | Graphs *still* don't join | Backmerge with a **merge commit** |
| Date changelog on `develop` | Rule 1 violation | Rule 3 — date it on `release/*` |
| `delete_branch_on_merge` enabled | `develop` deleted by a sync PR | Rule 4 — keep it disabled |
| PR body via `--body` with backticks | Shell mangles the markdown | `gh pr create --body-file <file>` |
| Tag on `develop` instead of `main` | Publish doesn't fire / fires on wrong code | Tag the `main` merge commit |

## Enforcement

- **Branch protection** on GitHub for `main` and `develop`: require PR + passing CI,
  no direct pushes, no deletions.
- Optional local `pre-push` hook (per clone) blocks pushes to protected branches:

```bash
cat > .git/hooks/pre-push <<'EOF'
#!/usr/bin/env bash
while read local_ref local_sha remote_ref remote_sha; do
  case "$remote_ref" in
    refs/heads/develop|refs/heads/main)
      echo "ia-gitflow: direct push to $remote_ref blocked (use a PR)." >&2
      exit 1 ;;
  esac
done
EOF
chmod +x .git/hooks/pre-push
```

## Verification before claiming done

The pre-commit hook runs these; CI enforces them. Run them yourself before opening a PR:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test        # includes the official spec suite
```
