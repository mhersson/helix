#!/usr/bin/env bash
# integrate.sh - Rebuild master from upstream with custom features and PRs
#
# This script recreates the master branch by:
# 1. Ensuring prerequisites (upstream remote, local my-changes branch)
# 2. Starting from upstream/master
# 3. Fetching and updating PR branches from GitHub
# 4. Merging desired PRs (if not yet merged upstream)
# 5. Cherry-picking custom commits from my-changes branch
# 6. Running post-integration verification (fmt, clippy)
#
# NOTE: The script will stop on merge conflicts. Resolve conflicts, complete
# the merge, then continue the remaining steps manually. PR-specific fixes
# are documented in the PR CONFIGURATION section below — apply them after
# merging the relevant PR.
#
# Usage: ./integrate.sh [--dry-run]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# === PR CONFIGURATION ===
# Format: [PR_NUMBER]="description"
declare -A PRS=(
  [14876]="feat(lsp): textDocument/inlineCompletion support"
  [13133]="feat: Inline Git Blame"
)

# Order matters - PRs are merged in this order
PR_ORDER=(14876 13133)

# === PR-SPECIFIC FIXES ===
# Fixes to apply manually after merging each PR.
# These cannot be automated as patch files because the integration branch
# starts from upstream/master where local files don't exist yet.
#
# PR #13133: Add missing #[cfg(feature = "git")] guard in helix-vcs/src/lib.rs
#   Before: pub use git::blame::FileBlame;
#   After:  #[cfg(feature = "git")]
#           pub use git::blame::FileBlame;
#   Commit message: fix(vcs): add missing cfg feature gate for FileBlame re-export

# === END CONFIGURATION ===

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Parse arguments
DRY_RUN=""
if [ "$1" == "--dry-run" ]; then
  DRY_RUN="true"
  echo -e "${YELLOW}=== DRY RUN MODE ===${NC}"
fi

# Ensure upstream remote exists
ensure_upstream_remote() {
  if ! git remote get-url upstream &>/dev/null; then
    echo -e "${YELLOW}Adding upstream remote...${NC}"
    git remote add upstream https://github.com/helix-editor/helix.git
    echo -e "  ${GREEN}Added upstream -> https://github.com/helix-editor/helix.git${NC}"
  fi
}

# Ensure local my-changes branch exists
ensure_my_changes_branch() {
  if ! git show-ref --verify --quiet refs/heads/my-changes; then
    if git show-ref --verify --quiet refs/remotes/origin/my-changes; then
      echo -e "${YELLOW}Creating local my-changes branch from origin/my-changes...${NC}"
      git branch my-changes origin/my-changes
      echo -e "  ${GREEN}Created my-changes branch${NC}"
    else
      echo -e "${RED}ERROR: No my-changes branch found locally or on origin${NC}"
      exit 1
    fi
  fi
}

# Fetch and update all PR branches from GitHub
fetch_and_update_prs() {
  echo -e "\n${YELLOW}Fetching PR branches from GitHub...${NC}"

  for pr_num in "${PR_ORDER[@]}"; do
    local branch="pr-${pr_num}"
    local refspec="refs/pull/${pr_num}/head"

    echo -n "  PR #${pr_num}: "

    # Fetch the PR ref from upstream
    if ! git fetch upstream "$refspec" 2>/dev/null; then
      echo -e "${RED}FAILED to fetch${NC}"
      continue
    fi

    # Check if local branch exists
    if git show-ref --verify --quiet "refs/heads/${branch}"; then
      # Update existing branch
      local local_sha=$(git rev-parse "${branch}")
      local remote_sha=$(git rev-parse FETCH_HEAD)

      if [ "$local_sha" == "$remote_sha" ]; then
        echo -e "${GREEN}up to date${NC}"
      else
        git update-ref "refs/heads/${branch}" FETCH_HEAD
        echo -e "${GREEN}updated${NC} (${local_sha:0:8} -> ${remote_sha:0:8})"
      fi
    else
      # Create new branch
      git branch "$branch" FETCH_HEAD
      echo -e "${GREEN}created${NC}"
    fi
  done
}

# Rebase my-changes onto upstream/master
rebase_my_changes() {
  echo -e "\n${YELLOW}Rebasing my-changes onto upstream/master...${NC}"

  local original_branch
  original_branch=$(git symbolic-ref --short HEAD 2>/dev/null || git rev-parse --short HEAD)

  local behind
  behind=$(git rev-list --count my-changes..upstream/master 2>/dev/null || echo "?")
  echo "  my-changes is ${behind} commits behind upstream/master"

  if git rebase upstream/master my-changes; then
    echo -e "  ${GREEN}Rebase successful${NC}"
    git checkout "$original_branch" --quiet
  else
    echo -e "\n${RED}Rebase conflict!${NC}"
    echo "Resolve conflicts, then run: git rebase --continue"
    echo "After the rebase completes, re-run this script."
    exit 1
  fi
}

# Check if a PR is merged into upstream/master
# Usage: check_pr_merged PR_NUMBER
# Returns: 0 if merged, 1 if not merged
check_pr_merged() {
  local pr_num="$1"
  local branch="pr-${pr_num}"

  # Check if the PR branch tip is an ancestor of upstream/master
  # This means all commits from the PR are in upstream
  if git merge-base --is-ancestor "$branch" upstream/master 2>/dev/null; then
    return 0 # merged
  else
    return 1 # not merged
  fi
}

echo -e "${GREEN}=== Helix Fork Integration Script ===${NC}"

# Ensure prerequisites
ensure_upstream_remote
ensure_my_changes_branch

# Fetch latest upstream
echo -e "\n${YELLOW}Fetching upstream...${NC}"
git fetch upstream

# Fetch and update PR branches
fetch_and_update_prs

# Check PR status and store results
echo -e "\n${YELLOW}Checking PR status...${NC}"
declare -A PR_MERGED
for pr_num in "${PR_ORDER[@]}"; do
  if check_pr_merged "$pr_num"; then
    PR_MERGED[$pr_num]=true
    echo -e "  PR #${pr_num} (${PRS[$pr_num]}): ${GREEN}MERGED${NC}"
  else
    PR_MERGED[$pr_num]=false
    echo -e "  PR #${pr_num} (${PRS[$pr_num]}): ${YELLOW}NOT MERGED${NC}"
  fi
done

# Dry run - show what would happen
if [ -n "$DRY_RUN" ]; then
  echo -e "\n${YELLOW}=== What would happen ===${NC}"
  behind=$(git rev-list --count my-changes..upstream/master 2>/dev/null || echo "?")
  echo "1. Rebase my-changes onto upstream/master (currently ${behind} commits behind)"
  echo "2. Create 'integration' branch from upstream/master"

  step=3
  for pr_num in "${PR_ORDER[@]}"; do
    if [ "${PR_MERGED[$pr_num]}" == "false" ]; then
      echo "${step}. Merge PR #${pr_num} branch: pr-${pr_num}"
      ((step++))
    else
      echo "${step}. SKIP PR #${pr_num} (already merged)"
      ((step++))
    fi
  done

  echo "${step}. Cherry-pick custom commits from my-changes branch"
  ((step++))
  echo "${step}. Replace master with integration branch"
  ((step++))
  echo "${step}. Run cargo fmt --check and cargo clippy"
  exit 0
fi

# Confirm before proceeding
echo -e "\n${YELLOW}This will reset master to a new integration. Continue? [y/N]${NC}"
read -r response
if [[ ! "$response" =~ ^[Yy]$ ]]; then
  echo "Aborted."
  exit 1
fi

# Rebase my-changes onto upstream/master
rebase_my_changes

# Create integration branch
echo -e "\n${YELLOW}Creating integration branch...${NC}"
git checkout -B integration upstream/master

# Merge PRs
for pr_num in "${PR_ORDER[@]}"; do
  if [ "${PR_MERGED[$pr_num]}" == "false" ]; then
    branch="pr-${pr_num}"

    echo -e "\n${YELLOW}Merging PR #${pr_num}...${NC}"
    git merge --no-ff "$branch" -m "Merge upstream PR #${pr_num}: ${PRS[$pr_num]}"
  fi
done

# Cherry-pick custom commits from my-changes
echo -e "\n${YELLOW}Applying custom commits from my-changes...${NC}"

# Get the commits unique to my-changes (on top of upstream/master)
CUSTOM_COMMITS=$(git log upstream/master..my-changes --reverse --pretty=format:"%H" 2>/dev/null)

if [ -n "$CUSTOM_COMMITS" ]; then
  for commit in $CUSTOM_COMMITS; do
    COMMIT_MSG=$(git log --format="%s" -n 1 "$commit")
    echo "  Cherry-picking: $COMMIT_MSG"
    git cherry-pick "$commit"
  done
else
  echo "  No custom commits to apply"
fi

# Update master
echo -e "\n${YELLOW}Updating master...${NC}"
git checkout master
git reset --hard integration
git branch -D integration

# Post-integration verification
echo -e "\n${YELLOW}Running post-integration checks...${NC}"

echo -n "  cargo fmt --check: "
if cargo fmt --check 2>/dev/null; then
  echo -e "${GREEN}OK${NC}"
else
  echo -e "${RED}FAILED${NC} - run 'cargo fmt' to fix"
fi

echo -n "  cargo clippy: "
if cargo clippy 2>&1 | grep -q "^warning:"; then
  echo -e "${YELLOW}WARNINGS${NC} - run 'cargo clippy' to review"
else
  echo -e "${GREEN}OK${NC}"
fi

# Final summary
echo -e "\n${GREEN}=== Integration complete! ===${NC}"
echo "Master is now up to date with:"
echo "  - upstream/master"

for pr_num in "${PR_ORDER[@]}"; do
  if [ "${PR_MERGED[$pr_num]}" == "false" ]; then
    echo "  - PR #${pr_num} (${PRS[$pr_num]})"
  fi
done

echo "  - Custom commits from my-changes"
echo ""
echo "Review any warnings above, then run 'cargo build' for a full build."
