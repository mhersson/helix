//! Tests against the installed `jj`. They are skipped when `jj` is not on `PATH`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use super::test_hook::{self, JjCommand};
use crate::{DiffProvider, DiffProviderRegistry, FileChange};

/// File names that must survive being passed to `jj` as filesets and in `ui.diff-formatter`.
const FILES: [&str; 3] = ["plain.txt", "it's here.txt", "with spaces.txt"];

/// A `jj` repository isolated from the user's configuration, with the same isolation applied to
/// the `jj` commands that the registry runs from this thread.
struct JjRepo {
    _temp: TempDir,
    root: PathBuf,
    envs: Vec<(String, String)>,
}

impl JjRepo {
    /// Returns `None` (and prints why) when `jj` is not installed.
    fn init(colocate: bool) -> Option<Self> {
        if Command::new("jj").arg("--version").output().is_err() {
            eprintln!("skipping: `jj` is not on PATH");
            return None;
        }

        let temp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let config = base.join("jj-config.toml");
        std::fs::write(&config, "").unwrap();
        let envs = vec![
            ("JJ_CONFIG".to_owned(), config.to_str().unwrap().to_owned()),
            ("JJ_USER".to_owned(), "helix-test".to_owned()),
            ("JJ_EMAIL".to_owned(), "test@helix.org".to_owned()),
        ];
        test_hook::set(JjCommand {
            program: None,
            envs: envs.iter().map(|(k, v)| (k.into(), v.into())).collect(),
        });

        let root = base.join("repo");
        let repo = Self {
            _temp: temp,
            root,
            envs,
        };
        let colocate = if colocate {
            "--colocate"
        } else {
            "--no-colocate"
        };
        repo.jj(&base, &["git", "init", colocate, "repo"]);
        assert_eq!(repo.root.join(".git").exists(), colocate == "--colocate");
        Some(repo)
    }

    fn jj(&self, cwd: &Path, args: &[&str]) {
        let output = Command::new("jj")
            .current_dir(cwd)
            .args(args)
            .envs(self.envs.iter().map(|(k, v)| (k, v)))
            .env_remove("GIT_DIR")
            .output()
            .expect("failed to run `jj`");
        assert!(
            output.status.success(),
            "`jj {args:?}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Commit every file of `FILES` with its committed content, then modify them in the working
    /// copy.
    fn commit_and_modify(&self) {
        for name in FILES {
            std::fs::write(self.root.join(name), committed(name)).unwrap();
        }
        self.jj(&self.root, &["commit", "-m", "initial"]);
        self.jj(&self.root, &["bookmark", "create", "-r", "@-", "main"]);
        for name in FILES {
            std::fs::write(self.root.join(name), "modified\n").unwrap();
        }
    }
}

fn committed(name: &str) -> String {
    format!("committed {name}\n")
}

fn check_registry(repo: &JjRepo, trust_full: bool, expect_jj: bool) {
    let mut registry = DiffProviderRegistry::default();
    for name in FILES {
        let file = repo.root.join(name);
        registry.add(&file, trust_full);
        assert_eq!(
            matches!(registry.provider_for(&file), Some(DiffProvider::JJ(_))),
            expect_jj,
            "{:?}",
            registry.provider_for(&file)
        );
        assert_eq!(
            String::from_utf8(registry.get_diff_base(&file).expect("a diff base")).unwrap(),
            committed(name),
            "{name}"
        );
    }

    let file = repo.root.join(FILES[0]);
    let head = registry
        .get_current_head_name(&file)
        .expect("a head name")
        .load_full();
    eprintln!("head name: {head}");
    if expect_jj {
        // The short change id of `@`, then the bookmark on its parent.
        let (change_id, bookmarks) = head.split_once(' ').expect("bookmarks");
        assert_eq!(change_id.len(), 8, "{head:?}");
        assert!(
            change_id.chars().all(|c| c.is_ascii_lowercase()),
            "{head:?}"
        );
        assert_eq!(bookmarks, "[main]", "{head:?}");
    } else {
        assert!(!head.is_empty());
    }

    let modified = RefCell::new(Vec::new());
    registry
        .provider_for(&file)
        .unwrap()
        .for_each_changed_file(|change| {
            if let FileChange::Modified { path } = change.unwrap() {
                modified
                    .borrow_mut()
                    .push(path.file_name().unwrap().to_str().unwrap().to_owned());
            }
            true
        })
        .unwrap();
    let mut modified = modified.into_inner();
    modified.sort();
    let mut expected = FILES.map(str::to_owned);
    expected.sort();
    assert_eq!(modified, expected);
}

#[test]
fn live_jj_repo() {
    let Some(repo) = JjRepo::init(false) else {
        return;
    };
    repo.commit_and_modify();
    check_registry(&repo, true, true);
    eprintln!("ran live jj test: non-colocated");
}

#[test]
fn live_colocated_jj_repo() {
    let Some(repo) = JjRepo::init(true) else {
        return;
    };
    repo.commit_and_modify();
    check_registry(&repo, true, true);
    eprintln!("ran live jj test: colocated");
}

#[cfg(feature = "git")]
#[test]
fn live_untrusted_colocated_jj_repo_falls_back_to_git() {
    let Some(repo) = JjRepo::init(true) else {
        return;
    };
    repo.commit_and_modify();
    check_registry(&repo, false, false);
    eprintln!("ran live jj test: untrusted colocated, git fallback");
}
