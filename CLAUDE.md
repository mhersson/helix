# Helix Fork

This is a fork of helix-editor with custom features and merged upstream PRs.

## Code Quality

Both `cargo fmt` and `cargo clippy` **must** pass before every commit. Run:

```sh
cargo fmt
cargo clippy
```

## Branch Strategy

- `master` - Integrated branch (upstream + PRs + custom commits)
- `my-changes` - Clean commits that apply to upstream/master (push here for
  upstream-compatible work). Rebased onto upstream/master during integration.
- `pr-*` - Local copies of upstream PR branches (pr-13133, pr-14876)

## Updating from Upstream

Run `./integrate.sh` to rebuild master from upstream with PRs and custom
commits.

- Use `--dry-run` to preview what will happen
- The script auto-configures the `upstream` remote and local `my-changes`
  branch if missing
- Script will likely require manual conflict resolution (see below)
- PR-specific fixes are documented inline in integrate.sh (see the
  `PR-SPECIFIC FIXES` section) — apply them manually after merging the
  relevant PR

## Common Conflict Files

When merging PRs that add handlers/features:

- `helix-term/src/handlers.rs` - imports, module declarations, setup()
- `helix-view/src/handlers.rs` - event structs, Handlers struct fields
- `helix-vcs/Cargo.toml` - gix features (merge all needed features)
- `helix-core/src/syntax/config.rs` - LanguageServerFeature enum and Display impl
- `helix-lsp/src/client.rs` - supports_feature() and client capabilities
- `helix-view/src/document.rs` - Document struct field initialization
- `helix-term/src/events.rs` - event imports and register_event calls

### Conflict Resolution Strategy

When merging independent PRs or cherry-picking custom commits, the resolution
is almost always to **keep both sides** — each PR/commit adds its own
handlers, struct fields, imports, etc. that are independent of each other.

For `helix-vcs/Cargo.toml` gix features: use the **newer gix version** from
upstream and combine all feature flags from both sides.

**Always** run `cargo fmt` and `cargo clippy` after resolving conflicts before
committing.

## Integration Procedure (for Claude)

When running `./integrate.sh`, the script will stop on the first merge
conflict. After resolving, you must continue the remaining steps manually:

1. **Run the script** with `echo "y" | ./integrate.sh` — it will stop at the
   first conflict (rebase or merge)
2. **If rebase conflict** — the script rebases `my-changes` onto
   `upstream/master` first. Resolve conflicts (same "keep both sides"
   strategy), run `git rebase --continue` (repeat as needed), then re-run the
   script
3. **If merge conflict** — resolve conflicts, then `git add <files> && git
   commit --no-edit`
4. **Continue remaining merges** — run the next `git merge --no-ff pr-XXXX`
   commands from the script's PR_ORDER
5. **Apply PR-specific fixes** — check integrate.sh's `PR-SPECIFIC FIXES`
   section and apply any documented fixes after their PR is merged
6. **Cherry-pick custom commits** — `git cherry-pick` each commit from
   `git log upstream/master..my-changes --reverse --pretty=format:"%H"`
7. **Replace master** — `git checkout master && git reset --hard integration
   && git branch -D integration`
8. **Update Cargo.lock** — run `cargo check` (merging PRs adds dependencies
   not in upstream's lockfile), then `git add Cargo.lock && git commit`
9. **Fix code quality** — run `cargo fmt` and `cargo clippy`, fix any issues,
   commit each fix
10. **Force-push my-changes** — `git push --force origin my-changes` (rebase
    rewrote its history)
11. **Verify clean state** — `git status` must show no uncommitted changes on
    master when done

**Do NOT** run a blanket `cargo update` — use upstream's dependency versions.
Only run targeted `cargo update <package>` if a specific crate is yanked.

## Commit Ordering

In `my-changes`, feature commits should come before README/docs commits that
describe them.

## Building

`cargo build` or `cargo check --package helix-term` for faster iteration.

After integration, also run `cargo clippy` to catch warnings introduced by
combining PRs, and check for yanked packages in Cargo.lock.
