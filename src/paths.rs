use std::{
    env, io,
    path::{Path, PathBuf},
};

/// Monorepo source tree paths. All members other than `root` are relative to
/// `root`.
pub struct MonorepoPaths {
    /// The monorepo checkout root, as an absolute path.
    pub root: PathBuf,

    /// The third_party directory.
    pub third_party: PathBuf,
}

impl MonorepoPaths {
    /// Create the `MonorepoPath` resolver. Accesses the filesystem to get the
    /// checkout root.
    pub fn new() -> io::Result<MonorepoPaths> {
        let root_dir = find_repo_root()?;

        Ok(MonorepoPaths {
            root: root_dir.clone(),
            third_party: check_path(&root_dir, THIRD_PARTY_DIR)?,
        })
    }

    /// Create the `MonorepoPath` resolver with custom 3rd party path. Accesses the filesystem to
    /// get the checkout root.
    pub fn from_third_party_dir(third_party_path: &str) -> io::Result<MonorepoPaths> {
        let root_dir = find_repo_root()?;

        Ok(MonorepoPaths {
            root: root_dir.clone(),
            third_party: check_path(&root_dir, third_party_path)?,
        })
    }
}

fn check_path(root: &Path, p_str: &str) -> io::Result<PathBuf> {
    let p = Path::new(p_str);
    let full_path = root.join(p_str);
    if !full_path.exists() {
        return Err(io::Error::other(format!(
            "could not find {} under {} (invoked from monorepo?)",
            p.display(),
            root.display(),
        )));
    }

    Ok(full_path)
}

/// Traverse up the directory tree to find the monorepo root (contains `.git`)
pub fn find_repo_root() -> io::Result<PathBuf> {
    let mut current = env::current_dir()?;

    loop {
        if current.join(".git").is_dir() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(io::Error::other(
                "could not find monorepo root (invoked from monorepo?)",
            ));
        }
    }
}

pub fn path_to_abs(paths: &MonorepoPaths, path: &str) -> io::Result<PathBuf> {
    if !path.starts_with("//") {
        return Err(io::Error::other(
            "Monorepo canonical path must start with //",
        ));
    }

    Ok(paths.root.join(&path[2..]))
}

static THIRD_PARTY_DIR: &str = "third_party";
