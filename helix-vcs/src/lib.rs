//! `helix_vcs` provides types for working with diffs from a Version Control System (VCS).
//! Diffs are provided by `git` (the `git` feature) and Jujutsu (the `jj` feature, through the
//! `jj` CLI). A Jujutsu repository falls back to its git repository when `jj` can't be used
//! there, e.g. when it isn't installed or the workspace isn't fully trusted.

use anyhow::Result;
use arc_swap::ArcSwap;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "git")]
mod git;
#[cfg(feature = "git")]
pub use git::blame::FileBlame;
#[cfg(feature = "jj")]
mod jj;

mod diff;

pub use diff::{DiffHandle, Hunk};

mod status;

pub use status::FileChange;

/// Contains all active diff providers. Diff providers are compiled in via features: `git` and
/// `jj`, with a Jujutsu repository falling back to git when `jj` can't be used (see
/// [`Self::add`]).
#[derive(Default, Clone, Debug)]
pub struct DiffProviderRegistry {
    /// Repository root path mapped to their provider.
    ///
    /// When a root path cannot be found after having called `add_file`, it means there is no
    /// provider to speak of.
    providers: HashMap<Arc<Path>, DiffProvider>,
    /// Count the number of files added for a specific provider path.
    /// Providers themselves don't care about that, this is handled entirely in `Self::add_file`,
    /// without knowledge from the `Self::add_file_<provider>` methods.
    ///
    /// Note: it *could* happen that a provider for a path is changed without the number of
    /// associated files changing, e.g deleting a .git/ and initializing a .jj/ repo.
    counters: HashMap<Arc<Path>, u32>,
    /// Repository roots with a `.jj/` where `jj` could not be run (e.g. it is not installed), so
    /// that it is not spawned again for every file opened there. Cleared with the repository's
    /// counter, or by [`Self::reset`].
    jj_unusable: HashSet<Arc<Path>>,
}

/// Diff-related methods
impl DiffProviderRegistry {
    /// Get the given file from the VCS. This provides the unedited document as a "base"
    /// for a diff to be created.
    pub fn get_diff_base(&self, file: &Path) -> Option<Vec<u8>> {
        match self.provider_for(file)?.get_diff_base(file) {
            Ok(diff_base) => Some(diff_base),
            Err(err) => {
                log::debug!("{err:#?}");
                log::debug!("failed to open diff base for {}", file.display());
                None
            }
        }
    }

    /// Get the current name of the current [HEAD](https://stackoverflow.com/questions/2304087/what-is-head-in-git).
    pub fn get_current_head_name(&self, file: &Path) -> Option<Arc<ArcSwap<Box<str>>>> {
        match self.provider_for(file)?.get_current_head_name() {
            Ok(head_name) => Some(head_name),
            Err(err) => {
                log::debug!("{err:#?}");
                log::debug!("failed to obtain current head name for {}", file.display());
                None
            }
        }
    }

    /// Whether a file system event changes VCS state (e.g. a git HEAD moved) so that the diff
    /// bases of open documents must be refreshed.
    pub fn needs_reload(&self, fs_event: &helix_core::file_watcher::Event) -> bool {
        self.providers
            .values()
            .any(|provider| provider.needs_reload(fs_event))
    }

    /// Get paths that need to be watched for VCS state changes.
    /// These are paths like HEAD files that indicate branch/commit changes.
    /// The workspace path is used to determine if the VCS metadata is external.
    ///
    /// `trust_full` is the workspace-trust decision for `workspace`; repositories are opened with
    /// it exactly like in [`Self::add`]. Only git repositories are supported: for a JJ repository,
    /// this is the HEAD of the git repository that [`Self::add`] may fall back to (e.g. a
    /// colocated one). JJ providers themselves ignore HEAD changes.
    #[cfg_attr(not(feature = "git"), allow(unused))]
    pub fn get_watched_paths(&self, workspace: &Path, trust_full: bool) -> Vec<PathBuf> {
        match get_possible_provider(workspace) {
            #[cfg(feature = "git")]
            Some((repo_path, _)) => git::get_head_path(repo_path, trust_full)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether `file` uses a git provider whose HEAD file is `head` (a canonical path).
    #[cfg_attr(not(feature = "git"), allow(unused))]
    pub fn uses_head(&self, file: &Path, head: &Path) -> bool {
        match self.provider_for(file) {
            #[cfg(feature = "git")]
            Some(DiffProvider::Git { repo, .. }) => git::head_path(repo) == head,
            _ => false,
        }
    }

    /// Fire-and-forget changed file iteration. Runs everything in a background task. Keeps
    /// iteration until `on_change` returns `false`.
    pub fn for_each_changed_file(
        self,
        cwd: Arc<Path>,
        f: impl Fn(Result<FileChange>) -> bool + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            let Some(diff_provider) = self.provider_for(&cwd) else {
                return;
            };
            if let Err(err) = diff_provider.for_each_changed_file(&f) {
                f(Err(err));
            }
        });
    }
}

/// Creation and update methods
#[cfg_attr(not(any(feature = "git", feature = "jj")), allow(unused))]
impl DiffProviderRegistry {
    /// Register a provider (if any is found) for the given path.
    ///
    /// A `.jj/` is used through `jj` when the workspace is fully trusted and `jj` works there.
    /// Otherwise (untrusted, `jj` not installed or failing), the git repository found from the
    /// same root is used instead, e.g. in a colocated repository, at the given trust level. A
    /// repository that fell back to git switches to `jj` on a later `add` once that works, e.g.
    /// after the workspace became trusted.
    pub fn add(&mut self, path: &Path, trust_full: bool) {
        let Some((repo_path, provider)) = get_possible_provider(path) else {
            // Do nothing here: there is no path to use and so the actual methods to get infos
            // like `get_diff_base` just won't do anything since they won't find a source to
            // work with.
            log::debug!("Found no potential diff provider for {}", path.display());
            // Note: if a `.<vcs>/` dir is deleted, we may end up in a situation where we lose track
            // of a now unused provider. This is acceptable because it doesn't happen that often in
            // practice and people can just reload to force an update.
            //
            // If it becomes an issue in the future, we could fix it by recomputing the providers
            // for each stored paths here.
            return;
        };

        // Count every `add`, even a failed one: callers pair each `add` with a `remove` (e.g.
        // `:reload` and closing a document), so counting only successes would let a failed `add`
        // steal another document's count on `remove`.
        *self.counters.entry(Arc::from(repo_path)).or_default() += 1;
        let created = !self.providers.contains_key(repo_path);

        let result: Result<(Arc<Path>, PossibleDiffProvider)> = match provider {
            #[cfg(feature = "git")]
            PossibleDiffProvider::Git => self.add_file_git(repo_path, trust_full),
            #[cfg(feature = "jj")]
            PossibleDiffProvider::JJ => self.add_file_jj_or_git(repo_path, trust_full),
        };

        match result {
            Ok((_, prov)) => {
                // Only log at info level when adding a new provider
                if created {
                    log::info!(
                        "Added {prov:?} (repo: {}) from {}",
                        repo_path.display(),
                        path.display()
                    )
                } else {
                    log::debug!(
                        "Reused {prov:?} (repo: {}) for {}",
                        repo_path.display(),
                        path.display()
                    );
                }
            }
            Err(err) => log::debug!(
                "Failed to open repo at {} for {}: {:?}",
                repo_path.display(),
                path.display(),
                err
            ),
        }
    }

    /// Reload the provider for the given path.
    pub fn reload(&mut self, path: &Path, trust_full: bool) {
        self.remove(path);
        self.add(path, trust_full);
    }

    /// Re-check the provider used by `path` against the workspace's current trust, without
    /// touching the per-document counters: like in [`Self::add`], a git provider opened at another
    /// trust level is reopened, and a JJ provider is replaced by the git fallback (if there is a
    /// git repository) when untrusted.
    ///
    /// Trust revocation is only applied lazily by `add`, so call this before using a provider
    /// outside of an `add`.
    ///
    /// The branch is picked from the stored provider's type, not the detected one, so this never
    /// starts running `jj` for a git provider: neither when a `.jj/` appears later, nor for a git
    /// fallback whose workspace became trusted. Only `add` switches a fallback to `jj`. A JJ
    /// provider that is still trusted is kept without running `jj`.
    pub fn revalidate(&mut self, path: &Path, trust_full: bool) {
        let Some((repo_path, _)) = get_possible_provider(path) else {
            return;
        };
        let Some(provider) = self.providers.get(repo_path) else {
            return;
        };
        let result: Result<(Arc<Path>, PossibleDiffProvider)> = match *provider {
            #[cfg(feature = "git")]
            DiffProvider::Git { .. } => self.add_file_git(repo_path, trust_full),
            #[cfg(feature = "jj")]
            DiffProvider::JJ(_) => self.add_file_jj_or_git(repo_path, trust_full),
        };
        match result {
            Ok(_) => {}
            Err(err) => log::debug!(
                "Failed to revalidate repo at {} for {}: {:?}",
                repo_path.display(),
                path.display(),
                err
            ),
        }
    }

    /// Remove the given path from the provider cache. If it was the last one using it, this will
    /// free up the provider.
    pub fn remove(&mut self, path: &Path) {
        let Some((repo_path, _)) = get_possible_provider(path) else {
            return;
        };

        let Some(count) = self.counters.get_mut(repo_path) else {
            return;
        };

        *count -= 1;
        if *count == 0 {
            // Cleanup the provider when the last user disappears
            self.counters.remove(repo_path);
            self.providers.remove(repo_path);
            self.jj_unusable.remove(repo_path);

            // While reallocating is costly, in most sessions of Helix there will be one main
            // workspace and sometimes a jump to some temporary one (for example from a jump-to-def
            // in an LSP) that will be closed after some time. We want to avoid keeping unused
            // RAM for this.
            self.providers.shrink_to_fit();
            self.counters.shrink_to_fit();
        }
    }

    /// Clears the saved providers completely.
    pub fn reset(&mut self) {
        // NOTE: we keep the allocated memory for reuse since this is mostly used through
        // `reload_all`, in which case the underlying repository is most likely still there and the
        // user just wants a clean slate for diffs and other features.
        let Self {
            providers,
            counters,
            jj_unusable,
        } = self;
        providers.clear();
        counters.clear();
        jj_unusable.clear();
    }
}

/// Private methods
impl DiffProviderRegistry {
    fn provider_for(&self, path: &Path) -> Option<&DiffProvider> {
        let path = get_possible_provider(path)?.0;
        self.providers.get(path)
    }

    // Remove a provider, notably used when its trust setting has changed from the original setup.
    // The counter is kept: the documents using this repository still pair their `add` with a
    // `remove`.
    #[cfg_attr(not(any(feature = "git", feature = "jj")), allow(unused))]
    fn remove_provider(&mut self, repo_path: &Path) {
        self.providers.remove(repo_path);
    }

    /// Add the git repo to the known providers *if* it isn't already known.
    #[cfg(feature = "git")]
    fn add_file_git(
        &mut self,
        repo_path: &Path,
        trust_full: bool,
    ) -> Result<(Arc<Path>, PossibleDiffProvider)> {
        // Don't build a git repo object if there is already one for that path.
        if let Some((
            key,
            DiffProvider::Git {
                trust_full: originally_trusted,
                ..
            },
        )) = self.providers.get_key_value(repo_path)
        {
            if *originally_trusted == trust_full {
                return Ok((Arc::clone(key), PossibleDiffProvider::Git));
            } else {
                self.remove_provider(repo_path);
            }
        }

        match git::open_repo(repo_path, trust_full) {
            Ok(repo) => {
                let key = Arc::from(repo_path);
                self.providers.insert(
                    Arc::clone(&key),
                    DiffProvider::Git {
                        repo: Box::new(repo),
                        trust_full,
                    },
                );
                Ok((key, PossibleDiffProvider::Git))
            }
            Err(err) => Err(err),
        }
    }

    /// Add the JJ repo to the known providers *if* it isn't already known.
    #[cfg(feature = "jj")]
    fn add_file_jj(
        &mut self,
        repo_path: &Path,
        trust_full: bool,
    ) -> Result<(Arc<Path>, PossibleDiffProvider)> {
        use anyhow::Context;

        if !trust_full {
            // Trust may have been revoked since the provider was created: stop using it. A git
            // fallback provider is kept for `add_file_jj_or_git` to reuse.
            if let Some(DiffProvider::JJ(_)) = self.providers.get(repo_path) {
                self.remove_provider(repo_path);
            }
            log::debug!(concat!(
                "JJ repositories are only supported when the repository is trusted in Helix",
                " because JJ itself does not expose a trust setting yet.\n",
                "\n",
                "See <https://docs.jj-vcs.dev/latest/design/secure-config/> and ",
                "<https://docs.jj-vcs.dev/latest/design/managed-config/>."
            ));
            anyhow::bail!("Only fully trusted workspaces are supported for JJ repositories");
        }
        // Don't build a JJ repo object if there is already one for that path.
        if let Some((key, DiffProvider::JJ(_))) = self.providers.get_key_value(repo_path) {
            return Ok((Arc::clone(key), PossibleDiffProvider::JJ));
        }
        if self.jj_unusable.contains(repo_path) {
            anyhow::bail!("`jj` already failed for this repository");
        }

        // Providers are keyed and looked up by the path as found by `get_possible_provider`, but
        // `jj` is run on the canonical path.
        let canonical_repo_path = std::fs::canonicalize(repo_path)
            .context("failed to canonicalize potential jj repo path")?;

        match jj::open_repo(&canonical_repo_path) {
            Ok(()) => {
                // This replaces a git fallback provider, if any.
                let key = Arc::from(repo_path);
                self.providers.insert(
                    Arc::clone(&key),
                    DiffProvider::JJ(Arc::from(canonical_repo_path)),
                );
                Ok((key, PossibleDiffProvider::JJ))
            }
            Err(err) => {
                self.jj_unusable.insert(Arc::from(repo_path));
                Err(err)
            }
        }
    }

    /// Add the JJ repo at `repo_path`, or fall back to the git repository found from there if
    /// `jj` can't be used (see [`Self::add`]).
    #[cfg(feature = "jj")]
    fn add_file_jj_or_git(
        &mut self,
        repo_path: &Path,
        trust_full: bool,
    ) -> Result<(Arc<Path>, PossibleDiffProvider)> {
        let jj_err = match self.add_file_jj(repo_path, trust_full) {
            Ok(added) => return Ok(added),
            Err(err) => err,
        };
        #[cfg(feature = "git")]
        match self.add_file_git(repo_path, trust_full) {
            Ok(added) => {
                log::debug!(
                    "Using git for JJ repo at {}: {jj_err:#}",
                    repo_path.display()
                );
                Ok(added)
            }
            Err(git_err) => Err(jj_err.context(format!("git fallback failed: {git_err:#}"))),
        }
        #[cfg(not(feature = "git"))]
        Err(jj_err)
    }
}

/// A union type that includes all types that implement [DiffProvider]. We need this type to allow
/// cloning [DiffProviderRegistry] as `Clone` cannot be used in trait objects.
#[derive(Clone, Debug)]
pub enum DiffProvider {
    #[cfg(feature = "git")]
    Git {
        repo: Box<gix::ThreadSafeRepository>,
        trust_full: bool,
    },
    /// For [`jujutsu`](https://github.com/martinvonz/jj), we don't use the library but instead we
    /// call the binary because it can dynamically load backends, which the JJ library doesn't know about.
    #[cfg(feature = "jj")]
    JJ(Arc<Path>),
}

#[cfg_attr(not(any(feature = "git", feature = "jj")), allow(unused))]
impl DiffProvider {
    #[cfg_attr(not(feature = "git"), allow(unused_variables))]
    fn needs_reload(&self, fs_event: &helix_core::file_watcher::Event) -> bool {
        match *self {
            #[cfg(feature = "git")]
            Self::Git { .. } => git::is_head_path(fs_event.path.as_std_path()),
            // JJ keeps its state in `.jj/`, which the file watcher ignores.
            #[cfg(feature = "jj")]
            Self::JJ(_) => false,
        }
    }

    fn get_diff_base(&self, file: &Path) -> Result<Vec<u8>> {
        // We need the */ref else we're matching on a reference and Rust considers all references
        // inhabited.
        match *self {
            #[cfg(feature = "git")]
            Self::Git { ref repo, .. } => git::get_diff_base(repo, file),
            #[cfg(feature = "jj")]
            Self::JJ(ref repo) => jj::get_diff_base(repo, file),
        }
    }

    fn get_current_head_name(&self) -> Result<Arc<ArcSwap<Box<str>>>> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git { ref repo, .. } => git::get_current_head_name(repo),
            #[cfg(feature = "jj")]
            Self::JJ(ref repo) => jj::get_current_head_name(repo),
        }
    }

    fn for_each_changed_file(&self, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
        match *self {
            #[cfg(feature = "git")]
            Self::Git { ref repo, .. } => git::for_each_changed_file(repo, f),
            #[cfg(feature = "jj")]
            Self::JJ(ref repo) => jj::for_each_changed_file(repo, f),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum PossibleDiffProvider {
    /// Possibly a git repo rooted at the stored path (i.e. `<path>/.git` exists)
    #[cfg(feature = "git")]
    Git,
    /// Possibly a git repo rooted at the stored path (i.e. `<path>/.jj` exists)
    #[cfg(feature = "jj")]
    JJ,
}

/// Does *possible* diff provider auto detection. Returns the 'root' of the workspace
///
/// We say possible because this function doesn't open the actual repository to check if that's
/// actually the case.
fn get_possible_provider(path: &Path) -> Option<(&Path, PossibleDiffProvider)> {
    // TODO(poliorcetics): make checking order configurable
    let checks: &[(&str, PossibleDiffProvider)] = &[
        #[cfg(feature = "jj")]
        (".jj", PossibleDiffProvider::JJ),
        #[cfg(feature = "git")]
        (".git", PossibleDiffProvider::Git),
    ];

    if !checks.is_empty() {
        for parent in path.ancestors() {
            for &(repo_indic, pdp) in checks {
                if let Ok(true) = parent.join(repo_indic).try_exists() {
                    return Some((parent, pdp));
                }
            }
        }
    }

    None
}

#[cfg(all(test, feature = "jj"))]
mod test {
    use super::*;

    #[test]
    fn jj_provider_is_dropped_when_trust_is_revoked() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        std::fs::create_dir(repo.join(".jj")).unwrap();
        let file = repo.join("file.txt");

        // Simulate a provider opened while the workspace was trusted, used by one document.
        let mut registry = DiffProviderRegistry::default();
        let key: Arc<Path> = Arc::from(repo);
        registry
            .providers
            .insert(Arc::clone(&key), DiffProvider::JJ(Arc::clone(&key)));
        registry.counters.insert(Arc::clone(&key), 1);
        assert!(registry.provider_for(&file).is_some());

        // Opening another document after trust was revoked drops the provider.
        registry.add(&file, false);
        assert!(registry.provider_for(&file).is_none());
        assert_eq!(registry.counters.get(repo), Some(&2));

        // A failed `:reload` keeps the count balanced.
        registry.reload(&file, false);
        assert_eq!(registry.counters.get(repo), Some(&2));

        // Closing both documents clears the counter.
        registry.remove(&file);
        assert_eq!(registry.counters.get(repo), Some(&1));
        registry.remove(&file);
        assert!(!registry.counters.contains_key(repo));
        assert!(registry.providers.is_empty());
    }

    #[test]
    fn revalidate_drops_jj_provider_when_trust_is_revoked() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        std::fs::create_dir(repo.join(".jj")).unwrap();
        let file = repo.join("file.txt");

        // A provider opened while the workspace was trusted, used by one document.
        let mut registry = DiffProviderRegistry::default();
        let key: Arc<Path> = Arc::from(repo);
        registry
            .providers
            .insert(Arc::clone(&key), DiffProvider::JJ(Arc::clone(&key)));
        registry.counters.insert(Arc::clone(&key), 1);

        // A JJ repo never reacts to a git HEAD change.
        assert!(!registry.uses_head(&file, &repo.join(".git/HEAD")));

        // Still trusted: the provider is kept as is (and `jj` is not run to reopen it).
        registry.revalidate(&file, true);
        assert!(registry.provider_for(&file).is_some());

        // After trust was revoked the provider is dropped, so `get_diff_base` can't run `jj`;
        // the document's count is untouched.
        registry.revalidate(&file, false);
        assert!(registry.provider_for(&file).is_none());
        assert!(registry.get_diff_base(&file).is_none());
        assert_eq!(registry.counters.get(repo), Some(&1));
    }
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use super::*;

    #[test]
    fn revalidate_downgrades_git_provider_when_trust_is_revoked() {
        let temp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(temp.path()).unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("init")
            .env_remove("GIT_DIR")
            .output()
            .expect("failed to run `git init`")
            .status;
        assert!(status.success());
        let file = repo.join("file.txt");
        std::fs::write(&file, "foo").unwrap();

        let mut registry = DiffProviderRegistry::default();
        registry.add(&file, true);
        let is_trusted = |registry: &DiffProviderRegistry| match registry.provider_for(&file) {
            Some(DiffProvider::Git { trust_full, .. }) => Some(*trust_full),
            _ => None,
        };
        assert_eq!(is_trusted(&registry), Some(true));

        assert!(registry.uses_head(&file, &repo.join(".git/HEAD")));
        assert!(!registry.uses_head(&file, &repo.join(".git/ORIG_HEAD")));

        // Trust was revoked: the repository is reopened at reduced trust (no smudge filters),
        // and the document's count is untouched.
        registry.revalidate(&file, false);
        assert_eq!(is_trusted(&registry), Some(false));
        assert_eq!(registry.counters.get(repo.as_path()), Some(&1));
    }

    #[test]
    fn test_needs_reload_regular_git() {
        use helix_core::file_watcher::{CanonicalPathBuf, Event, EventType};
        use std::path::Path;

        // Regular .git/HEAD should trigger reload
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new("/home/user/repo/.git/HEAD")),
            ty: EventType::Modified,
        };
        assert!(git::is_head_path(event.path.as_std_path()));

        // Other .git files should not trigger reload
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new("/home/user/repo/.git/config")),
            ty: EventType::Modified,
        };
        assert!(!git::is_head_path(event.path.as_std_path()));
    }

    #[test]
    fn test_needs_reload_worktree_head() {
        use helix_core::file_watcher::{CanonicalPathBuf, Event, EventType};
        use std::path::Path;

        // Worktree HEAD at .git/worktrees/<name>/HEAD should trigger reload
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new(
                "/home/user/main-repo/.git/worktrees/my-worktree/HEAD",
            )),
            ty: EventType::Modified,
        };
        assert!(git::is_head_path(event.path.as_std_path()));

        // Nested worktree name should also work
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new(
                "/home/user/main-repo/.git/worktrees/feature-branch/HEAD",
            )),
            ty: EventType::Modified,
        };
        assert!(git::is_head_path(event.path.as_std_path()));

        // Non-HEAD files in worktrees should not trigger reload
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new(
                "/home/user/main-repo/.git/worktrees/my-worktree/index",
            )),
            ty: EventType::Modified,
        };
        assert!(!git::is_head_path(event.path.as_std_path()));

        // HEAD files not in .git/worktrees should not trigger reload
        let event = Event {
            path: CanonicalPathBuf::assert_canonicalized(Path::new(
                "/home/user/other/worktrees/my-worktree/HEAD",
            )),
            ty: EventType::Modified,
        };
        assert!(!git::is_head_path(event.path.as_std_path()));
    }
}

#[cfg(all(test, feature = "jj", feature = "git"))]
mod fallback_tests {
    use super::*;
    use crate::jj::test_hook::{self, JjCommand};

    /// A colocated-looking repository: `git init` plus an empty `.jj/`.
    fn colocated_repo() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(temp.path()).unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("init")
            .env_remove("GIT_DIR")
            .output()
            .expect("failed to run `git init`")
            .status;
        assert!(status.success());
        std::fs::create_dir(repo.join(".jj")).unwrap();
        let file = repo.join("file.txt");
        std::fs::write(&file, "foo").unwrap();
        (temp, file)
    }

    /// Run `program` instead of `jj` from this thread.
    fn use_jj_program(program: &str) {
        test_hook::set(JjCommand {
            program: Some(program.into()),
            envs: Vec::new(),
        });
    }

    fn git_trust(registry: &DiffProviderRegistry, file: &Path) -> Option<bool> {
        match registry.provider_for(file) {
            Some(DiffProvider::Git { trust_full, .. }) => Some(*trust_full),
            _ => None,
        }
    }

    fn is_jj(registry: &DiffProviderRegistry, file: &Path) -> bool {
        matches!(registry.provider_for(file), Some(DiffProvider::JJ(_)))
    }

    #[test]
    fn colocated_repo_falls_back_to_git_when_jj_is_missing() {
        let (_temp, file) = colocated_repo();
        let repo = file.parent().unwrap();
        use_jj_program("/nonexistent/helix-test-jj");

        let mut registry = DiffProviderRegistry::default();
        registry.add(&file, true);
        assert_eq!(git_trust(&registry, &file), Some(true));
        assert!(registry.jj_unusable.contains(repo));
        // The git fallback's HEAD is watched and used like any git provider's.
        assert_eq!(
            registry.get_watched_paths(repo, true),
            [repo.join(".git/HEAD")]
        );
        assert!(registry.uses_head(&file, &repo.join(".git/HEAD")));

        // `jj` is not tried again: even a working one isn't run until the repo is released.
        use_jj_program("true");
        registry.add(&file, true);
        assert_eq!(git_trust(&registry, &file), Some(true));
        assert_eq!(registry.counters.get(repo), Some(&2));

        registry.remove(&file);
        registry.remove(&file);
        assert!(registry.providers.is_empty());
        assert!(registry.jj_unusable.is_empty());
    }

    #[test]
    fn untrusted_colocated_repo_uses_git_at_reduced_trust() {
        let (_temp, file) = colocated_repo();
        let repo = file.parent().unwrap();
        use_jj_program("/nonexistent/helix-test-jj");

        let mut registry = DiffProviderRegistry::default();
        registry.add(&file, false);
        assert_eq!(git_trust(&registry, &file), Some(false));
        // `jj` was not run at all.
        assert!(registry.jj_unusable.is_empty());
        assert_eq!(registry.counters.get(repo), Some(&1));
    }

    #[test]
    fn jj_repo_without_git_has_no_provider_when_jj_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(repo.join(".jj")).unwrap();
        let file = repo.join("file.txt");
        use_jj_program("/nonexistent/helix-test-jj");

        let mut registry = DiffProviderRegistry::default();
        registry.add(&file, true);
        assert!(registry.provider_for(&file).is_none());
        assert!(registry.get_watched_paths(&repo, true).is_empty());
        assert_eq!(registry.counters.get(repo.as_path()), Some(&1));
        registry.remove(&file);
        assert!(registry.counters.is_empty());
    }

    // `true` stands in for a working `jj`: `jj workspace root` only needs to succeed.
    #[cfg(unix)]
    #[test]
    fn fallback_follows_trust_changes() {
        let (_temp, file) = colocated_repo();
        let repo = file.parent().unwrap();
        use_jj_program("true");

        let mut registry = DiffProviderRegistry::default();
        registry.add(&file, false);
        assert_eq!(git_trust(&registry, &file), Some(false));

        // A HEAD-triggered revalidation of the git fallback never starts running `jj`, even once
        // trusted: with a missing `jj`, a run would mark it unusable.
        use_jj_program("/nonexistent/helix-test-jj");
        registry.revalidate(&file, true);
        assert_eq!(git_trust(&registry, &file), Some(true));
        assert!(registry.jj_unusable.is_empty());

        // Trust granted: the next `add` switches to `jj`.
        use_jj_program("true");
        registry.add(&file, true);
        assert!(is_jj(&registry, &file));
        assert!(!registry.uses_head(&file, &repo.join(".git/HEAD")));

        // Still trusted: the JJ provider is kept, without running `jj`.
        use_jj_program("/nonexistent/helix-test-jj");
        registry.revalidate(&file, true);
        assert!(is_jj(&registry, &file));
        assert!(registry.jj_unusable.is_empty());

        // Trust revoked: the JJ provider falls back to git at reduced trust.
        registry.revalidate(&file, false);
        assert_eq!(git_trust(&registry, &file), Some(false));

        // And back to `jj` via `add`, then to git via `add` again.
        use_jj_program("true");
        registry.add(&file, true);
        assert!(is_jj(&registry, &file));
        registry.add(&file, false);
        assert_eq!(git_trust(&registry, &file), Some(false));

        // The provider swaps never touched the per-document counts.
        assert_eq!(registry.counters.get(repo), Some(&4));
        for _ in 0..4 {
            registry.remove(&file);
        }
        assert!(registry.providers.is_empty());
        assert!(registry.counters.is_empty());
    }
}
