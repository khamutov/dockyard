use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::ExtractPatchCommandArgs;
use crate::UpdateCommandArgs;
use crate::VendorCommandArgs;
use crate::paths;
use anyhow::Context;
use anyhow::bail;
use anyhow::{Result, anyhow};
use dockyard::paths::MonorepoPaths;
use dockyard::paths::path_to_abs;
use dockyard::utils::run_command;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Clone)]
struct DependencyMetadata {
    url: String,
    version: String,
    update_state: Option<UpdateState>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
enum PatchState {
    Pending,
    Applied,
    Conflict,
    Resolved,
}

impl Display for PatchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchState::Pending => write!(f, "Pending"),
            PatchState::Conflict => write!(f, "Conflict"),
            PatchState::Applied => write!(f, "Applied"),
            PatchState::Resolved => write!(f, "Resolved"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct PatchApplyState {
    name: String,
    state: PatchState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct UpdateState {
    prev_commit_hash: String,
    patches: Vec<PatchApplyState>,
}

const DEP_INFO: &str = "dep_info.json";

pub fn vendor(args: VendorCommandArgs, paths: &paths::MonorepoPaths) -> Result<()> {
    let target_dir = path_to_abs(paths, &args.path)?;

    if target_dir.exists() {
        return Err(anyhow!("Target must be empty: {}", target_dir.display()));
    }
    fs::create_dir_all(&target_dir)?;

    let clone_dir = target_dir.join("repo");
    let mut clone_cmd = Command::new("git");
    clone_cmd.args(["clone", &args.git, clone_dir.to_str().unwrap()]);

    run_command(clone_cmd, "clone", None).context("Failed to clone repo")?;

    // Checkout version if it's provided (tag/branch/commit)
    let version_str = if let Some(version) = args.version {
        let mut checkout_version_cmd = Command::new("git");
        checkout_version_cmd
            .current_dir(&clone_dir)
            .args(["checkout", &version]);
        run_command(checkout_version_cmd, "clone", None)
            .context("Failed to checkout specific version")?;
        version
    } else {
        let version_cmd = Command::new("git")
            .current_dir(&clone_dir)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !version_cmd.status.success() {
            return Err(anyhow!("git rev-parse failed"));
        }
        String::from_utf8(version_cmd.stdout)?.trim().to_string()
    };
    fs::remove_dir_all(clone_dir.join(".git"))?;

    let meta = DependencyMetadata {
        url: args.git.to_string(),
        version: version_str.to_string(),
        update_state: None,
    };
    update_metadata(&target_dir, &meta)?;

    Ok(())
}

fn update_metadata(target_dir: &PathBuf, metadata: &DependencyMetadata) -> Result<()> {
    let json = serde_json::to_string_pretty(&metadata)?;
    fs::write(target_dir.join(DEP_INFO), json)?;

    Ok(())
}

fn load_metadata(target_dir: &PathBuf) -> Result<DependencyMetadata> {
    let file = File::open(target_dir.join(DEP_INFO))?;
    let reader = BufReader::new(file);

    let metadata: DependencyMetadata = serde_json::from_reader(reader)?;

    Ok(metadata)
}

fn get_update_version(args: &UpdateCommandArgs, metadata: &DependencyMetadata) -> Result<String> {
    if let Some(ref version) = args.version {
        Ok(version.clone())
    } else {
        let version_cmd = Command::new("git")
            .args(["ls-remote", &metadata.url, "HEAD"])
            .output()?;
        if !version_cmd.status.success() {
            bail!(
                "git ls-remote failed, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&version_cmd.stdout),
                String::from_utf8_lossy(&version_cmd.stderr),
            );
        }
        let output = String::from_utf8(version_cmd.stdout)?.trim().to_string();

        // git ls-remote shows
        // commit_hash HEAD
        let mut iter = output.split_whitespace();
        if let Some(version) = iter.next() {
            Ok(version.to_string())
        } else {
            bail!("Unexpected git ls-remote output: {}", output);
        }
    }
}

pub fn get_current_commit() -> Result<String> {
    let version_cmd = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !version_cmd.status.success() {
        bail!("git rev-parse failed");
    }
    Ok(String::from_utf8(version_cmd.stdout)?.trim().to_string())
}

pub fn update(args: UpdateCommandArgs, paths: &paths::MonorepoPaths) -> Result<()> {
    ensure_git_clean(&paths.root)?;
    let canonical_path = &args.path.as_ref().unwrap();

    let target_dir = path_to_abs(paths, &canonical_path)?;

    if !target_dir.exists() {
        bail!("Target not found: {}", target_dir.display());
    }

    let mut metadata: DependencyMetadata = load_metadata(&target_dir)?;

    if args.status {
        if let Some(update_state) = metadata.update_state {
            println!("Active update state:");
            for (idx, patch) in update_state.patches.iter().enumerate() {
                println!("{}. {} - {}", idx + 1, patch.name, patch.state);
            }
        } else {
            println!("No active update");
        }

        return Ok(());
    }

    if args.cont {
        if metadata.update_state.is_none() {
            bail!("No active update state");
        }
        apply_patches(&target_dir, &canonical_path, paths, &mut metadata)?;

        metadata.update_state = None;
        update_metadata(&target_dir, &metadata)?;

        let commit_msg = format!("Update metadata for {}", &canonical_path);
        commit_code(&commit_msg, &paths.root)?;
        println!("All patches were applied");
        return Ok(());
    }

    let version = get_update_version(&args, &metadata)?;

    if version == metadata.version && !args.force {
        bail!("Already on the specified version");
    }

    let repo_dir = target_dir.join("repo");
    if !repo_dir.exists() && !args.force {
        bail!("Repo dir not found: {}", repo_dir.display());
    }

    fs::remove_dir_all(&repo_dir)?;

    let mut clone_cmd = Command::new("git");
    clone_cmd.args(["clone", &metadata.url, repo_dir.to_str().unwrap()]);
    run_command(clone_cmd, "clone", None).context("Failed to clone repo")?;

    let mut checkout_version_cmd = Command::new("git");
    checkout_version_cmd
        .current_dir(&repo_dir)
        .args(["checkout", &version]);

    fs::remove_dir_all(repo_dir.join(".git"))?;

    metadata.version = version.clone();
    metadata.update_state = Some(UpdateState {
        prev_commit_hash: get_current_commit()?,
        patches: load_patch_list(&target_dir)?
            .iter()
            .map(|e| PatchApplyState {
                name: e.clone(),
                state: PatchState::Pending,
            })
            .collect(),
    });
    update_metadata(&target_dir, &metadata)?;

    let commit_message = format!("Update {} to {}", &canonical_path, version);
    commit_code(&commit_message, &paths.root)?;

    apply_patches(&target_dir, &canonical_path, paths, &mut metadata)?;

    metadata.update_state = None;
    update_metadata(&target_dir, &metadata)?;

    let commit_msg = format!("Update metadata for {}", &canonical_path);
    commit_code(&commit_msg, &paths.root)?;
    println!("All patches were applied");
    Ok(())
}

fn apply_patches(
    target_dir: &PathBuf,
    canonical_path: &str,
    paths: &MonorepoPaths,
    metadata: &mut DependencyMetadata,
) -> Result<()> {
    let mut update_state_mut = metadata.update_state.clone().unwrap();

    if let Some(ref update_state) = metadata.clone().update_state {
        let patches_count = update_state.patches.len();
        println!("\nApplying patches:");
        for (idx, patch) in update_state.patches.clone().iter().enumerate() {
            match patch.state {
                PatchState::Pending => {
                    match try_apply_patch(target_dir, paths, &patch.name) {
                        Ok(_) => {
                            update_state_mut.patches[idx].state = PatchState::Applied;
                            metadata.update_state = Some(update_state_mut.clone());
                            update_metadata(target_dir, metadata)?;
                            let commit_msg = format!(
                                "Applied patch ({}/{}) {} for {}",
                                idx + 1,
                                patches_count,
                                patch.name,
                                &canonical_path,
                            );
                            commit_code(&commit_msg, &paths.root)?;
                            println!(
                                "Successfully applied patch ({}/{}) {} for {}",
                                idx + 1,
                                patches_count,
                                patch.name,
                                &canonical_path,
                            );
                        }
                        Err(_) => {
                            update_state_mut.patches[idx].state = PatchState::Conflict;
                            metadata.update_state = Some(update_state_mut.clone());
                            update_metadata(target_dir, metadata)?;

                            let relative_target_path = target_dir.strip_prefix(&paths.root)?;
                            print!(
                                "Patch cannot be applied. What to do next:

1. Try to apply with rejected hunks:

  cd {}
  git apply --reject --directory={}/repo ../patches/{}

2. Check *.rej files and apply conflicted hunks manually in source files (not in patch).
3. Run the following command

  dockyard update --continue {}

It'll refresh the current patch and will continue with subsequent patches.

",
                                relative_target_path.display(),
                                relative_target_path.display(),
                                patch.name,
                                canonical_path
                            );
                            bail!("Failed apply patch");
                        }
                    };
                }
                PatchState::Applied => {
                    println!("Skipping already applied patch {}", patch.name);
                }
                PatchState::Conflict => {
                    let repo_dir = target_dir.join("repo");
                    let diff = extract_diff(&repo_dir, paths)?;

                    let patches_dir = target_dir.join("patches");
                    let patch_path = patches_dir.join(&patch.name);

                    let mut file = File::create(&patch_path)?;
                    file.write_all(&diff)?;

                    println!("Patch {} updated", patch_path.display());

                    update_state_mut.patches[idx].state = PatchState::Resolved;
                    metadata.update_state = Some(update_state_mut.clone());
                    update_metadata(target_dir, metadata)?;
                    let commit_msg = format!(
                        "Resolve conflicted patch ({}/{}) {} for {}",
                        idx, patches_count, patch.name, &canonical_path,
                    );
                    commit_code(&commit_msg, &paths.root)?;
                }
                PatchState::Resolved => {
                    println!("Skipping already applied patch {}", patch.name);
                }
            };
        }
        Ok(())
    } else {
        bail!("No active update");
    }
}

fn try_apply_patch(
    target_dir: &PathBuf,
    paths: &paths::MonorepoPaths,
    patch_name: &str,
) -> Result<()> {
    let patches_dir = target_dir.join("patches");
    let repo_dir = target_dir.join("repo");
    let patch_path = patches_dir.join(&patch_name);
    let relative_path = repo_dir.strip_prefix(&paths.root)?;
    let relative_path = relative_path.to_string_lossy().replace('\\', "/");

    let dir_args = format!("--directory={}", &relative_path);
    let output = Command::new("git")
        .current_dir(&repo_dir)
        .args(["apply", "-3", &dir_args, &patch_path.to_string_lossy()])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        eprintln!("Patch failed");
        bail!("Patch failed {}", String::from_utf8_lossy(&output.stderr));
    }
}

fn git_add_all(current_dir: &Path) -> Result<()> {
    let git_cmd = Command::new("git")
        .current_dir(current_dir)
        .args(["add", "."])
        .output()?;

    if !git_cmd.status.success() {
        bail!(
            "git add failed, stdout: {}, stderr: {}",
            String::from_utf8_lossy(&git_cmd.stdout),
            String::from_utf8_lossy(&git_cmd.stderr),
        );
    }

    Ok(())
}

fn commit_code(message: &str, current_dir: &Path) -> Result<()> {
    git_add_all(&current_dir)?;

    let commit_cmd = Command::new("git")
        .current_dir(current_dir)
        .args(["commit", "-a", "-m", message])
        .output()?;

    if !commit_cmd.status.success() {
        bail!(
            "git commit failed, stdout: {}, stderr: {}",
            String::from_utf8_lossy(&commit_cmd.stdout),
            String::from_utf8_lossy(&commit_cmd.stderr),
        );
    }

    Ok(())
}

fn ensure_git_clean(current_dir: &Path) -> Result<()> {
    let git_cmd = Command::new("git")
        .current_dir(current_dir)
        .args(["status", "--porcelain"])
        .output()?;

    if !git_cmd.status.success() {
        bail!(
            "git status failed: stdout: {} stderr: {}",
            String::from_utf8_lossy(&git_cmd.stdout),
            String::from_utf8_lossy(&git_cmd.stderr),
        );
    }

    if git_cmd.stdout.len() > 0 {
        bail!(
            "git must be clean, but has changes:\n {}",
            String::from_utf8_lossy(&git_cmd.stdout),
        );
    }

    Ok(())
}

fn load_patch_list(target_dir: &PathBuf) -> Result<Vec<String>> {
    let patches_dir = target_dir.join("patches");

    let mut patches = Vec::new();
    for entry in fs::read_dir(&patches_dir)? {
        let entry = entry?;
        let fname = entry.file_name().into_string().unwrap();
        if let Some(n_str) = fname.split('-').next() {
            if let Ok(n) = n_str.parse::<u32>() {
                patches.push((n, fname));
            } else {
                bail!("");
            }
        } else {
            bail!("");
        }
    }

    patches.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(patches.iter().map(|e| e.1.clone()).collect())
}

pub fn extract_patch(args: ExtractPatchCommandArgs, paths: &paths::MonorepoPaths) -> Result<()> {
    let target_dir = path_to_abs(paths, &args.path)?;

    if !target_dir.exists() {
        return Err(anyhow!("Target doesn't exists: {}", target_dir.display()));
    }

    let repo_dir = target_dir.join("repo");
    let patches_dir = target_dir.join("patches");
    if !patches_dir.exists() {
        fs::create_dir_all(&patches_dir)?;
    }

    let diff = extract_diff(&repo_dir, paths)?;

    // Determine patch number
    let mut max_n = 0;
    for entry in fs::read_dir(&patches_dir)? {
        let entry = entry?;
        let fname = entry.file_name().into_string().unwrap();
        if let Some(n_str) = fname.split('-').next() {
            if let Ok(n) = n_str.parse::<u32>() {
                if n > max_n {
                    max_n = n;
                }
            }
        }
    }
    let patch_number = format!("{:04}", max_n + 1);
    let patch_name = format!("{patch_number}-change_name.patch");
    let patch_path = patches_dir.join(patch_name);

    let mut file = File::create(&patch_path)?;
    file.write_all(&diff)?;

    println!("Patch written to: {}", patch_path.display());

    Ok(())
}

fn extract_diff(repo_dir: &PathBuf, paths: &paths::MonorepoPaths) -> Result<Vec<u8>> {
    let relative_path = repo_dir.strip_prefix(&paths.root)?;

    let repo_dir = repo_dir.to_string_lossy().replace('\\', "/");
    let relative_path = relative_path.to_string_lossy().replace('\\', "/");

    let ls = Command::new("git")
        .current_dir(&paths.root)
        .args(["ls-files", "--others", "--exclude-standard", &repo_dir])
        .output()?;
    if !ls.status.success() {
        bail!(
            "git ls-files failed, stdout: {}, stderr: {}",
            String::from_utf8_lossy(&ls.stdout),
            String::from_utf8_lossy(&ls.stderr),
        );
    }

    if !ls.stdout.is_empty() {
        return Err(anyhow!("untracked files exist under {}", &repo_dir));
    }

    let patch_cmd = Command::new("git")
        .current_dir(&paths.root)
        .args([
            "diff".to_string(),
            // include all files (from index and unstaged)
            "HEAD".to_string(),
            format!("--relative={}", &relative_path),
            "--".to_string(),
            repo_dir.clone(),
        ])
        .output()?;

    if !patch_cmd.status.success() {
        return Err(anyhow!("git diff failed"));
    }

    if patch_cmd.stdout.is_empty() {
        return Err(anyhow!("no changes detected in third_party: {}", repo_dir));
    }

    Ok(patch_cmd.stdout)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use anyhow::{Context, bail};
    use dockyard::{paths::path_to_abs, *};
    use tempfile::{TempDir, tempdir};

    use super::*;

    #[test]
    fn test_extract_patch() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//third_party/repo_extract")?;
        fs::create_dir_all(&target_dir)?;
        fs::write(temp_dir.path().join(".keep"), "")?;
        commit_code("Initial commit", temp_dir.path())?;

        fs::write(target_dir.join("tesfile.txt"), "line1\nline2\n")?;

        git_add_all(&paths.root)?;
        let diff = extract_diff(&target_dir, &paths)?;

        let diff_str = String::from_utf8_lossy(&diff);

        let expected_diff = "diff --git a/tesfile.txt b/tesfile.txt
new file mode 100644
index 0000000..c0d0fb4
--- /dev/null
+++ b/tesfile.txt
@@ -0,0 +1,2 @@
+line1
+line2
";
        assert_eq!(diff_str, expected_diff);

        Ok(())
    }

    #[test]
    fn test_extract_patch_with_repo_subdir() -> anyhow::Result<()> {
        // Git expects forward slashes ('/') as the path separator. So let's check that the
        // functions works properly with possible backslashes after PahtBuf::join as well.
        let temp_dir = create_test_dir()?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//third_party/repo_extract_backslash")?;
        let repo_dir = target_dir.join("repo");
        fs::create_dir_all(&repo_dir)?;
        fs::write(temp_dir.path().join(".keep"), "")?;
        commit_code("Initial commit", temp_dir.path())?;

        fs::write(repo_dir.join("tesfile.txt"), "line1\nline2\n")?;

        git_add_all(&paths.root)?;
        let diff = extract_diff(&repo_dir, &paths)?;

        let diff_str = String::from_utf8_lossy(&diff);

        let expected_diff = "diff --git a/tesfile.txt b/tesfile.txt
new file mode 100644
index 0000000..c0d0fb4
--- /dev/null
+++ b/tesfile.txt
@@ -0,0 +1,2 @@
+line1
+line2
";
        assert_eq!(diff_str, expected_diff);

        Ok(())
    }

    #[test]
    fn test_extract_patch_error_on_untracked() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//third_party/repo_untracked")?;
        fs::create_dir_all(&target_dir)?;

        fs::write(target_dir.join("tesfile.txt"), "line1\nline2\n")?;

        let res = extract_diff(&target_dir, &paths);
        assert!(res.is_err(), "Expected Err, but get {:?}", res);

        Ok(())
    }

    #[test]
    fn test_update_apply_patches() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let target_dir = temp_dir.path().join("third_party/example");

        let mut metadata = DependencyMetadata {
            url: "empty".to_string(),
            version: "default".to_string(),
            update_state: None,
        };
        update_metadata(&target_dir, &metadata)?;

        fs::write(
            target_dir.join("repo/a.txt"),
            "line1
line2
line3
",
        )?;
        commit_code("Initial commit", &temp_dir.path())?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;

        let canonical_path = "//third_party/example";
        let target_dir = path_to_abs(&paths, canonical_path)?;

        fs::write(
            target_dir.join("patches/0001-update-line1.patch"),
            "diff --git a/a.txt b/a.txt
index 83db48f..efc6926 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line1
+line123
 line2
 line3
",
        )?;
        commit_code("Create patch1", &target_dir)?;

        // make all patches pending
        metadata.update_state = Some(UpdateState {
            prev_commit_hash: get_current_commit()?,
            patches: load_patch_list(&target_dir)?
                .iter()
                .map(|e| PatchApplyState {
                    name: e.clone(),
                    state: PatchState::Pending,
                })
                .collect(),
        });
        update_metadata(&target_dir, &metadata)?;

        apply_patches(&target_dir, canonical_path, &paths, &mut metadata)?;

        let new_metadata = load_metadata(&target_dir)?;
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[0].state,
            PatchState::Applied,
            "expected Applied state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );

        let content = fs::read_to_string(target_dir.join("repo/a.txt"))?;
        assert_eq!(
            content,
            "line123
line2
line3
"
        );

        Ok(())
    }

    #[test]
    fn test_update_apply_multiple_patches() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let target_dir = temp_dir.path().join("third_party/example");

        let mut metadata = DependencyMetadata {
            url: "empty".to_string(),
            version: "default".to_string(),
            update_state: None,
        };
        update_metadata(&target_dir, &metadata)?;

        fs::write(
            target_dir.join("repo/a.txt"),
            "line1
line2
line3
",
        )?;
        commit_code("Initial commit", &temp_dir.path())?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;

        let canonical_path = "//third_party/example";
        let target_dir = path_to_abs(&paths, canonical_path)?;

        fs::write(
            target_dir.join("patches/0001-update-line1.patch"),
            "diff --git a/a.txt b/a.txt
index 83db48f..efc6926 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line1
+line123
 line2
 line3
",
        )?;
        commit_code("Create patch1", &target_dir)?;

        fs::write(
            target_dir.join("patches/0002-update-line4.patch"),
            "diff --git a/a.txt b/a.txt
index 83db48f..efc6926 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line123
 line2
 line3
+line4
",
        )?;
        commit_code("Create patch2", &target_dir)?;

        // make all patches pending
        metadata.update_state = Some(UpdateState {
            prev_commit_hash: get_current_commit()?,
            patches: load_patch_list(&target_dir)?
                .iter()
                .map(|e| PatchApplyState {
                    name: e.clone(),
                    state: PatchState::Pending,
                })
                .collect(),
        });
        update_metadata(&target_dir, &metadata)?;

        apply_patches(&target_dir, canonical_path, &paths, &mut metadata)?;

        let new_metadata = load_metadata(&target_dir)?;
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[0].state,
            PatchState::Applied,
            "expected Applied state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[1].state,
            PatchState::Applied,
            "expected Applied state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );

        let content = fs::read_to_string(target_dir.join("repo/a.txt"))?;
        assert_eq!(
            content,
            "line2
line3
line4
"
        );

        Ok(())
    }

    #[test]
    fn test_update_apply_patch_with_conflict() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let target_dir = temp_dir.path().join("third_party/example");

        let mut metadata = DependencyMetadata {
            url: "empty".to_string(),
            version: "default".to_string(),
            update_state: None,
        };
        update_metadata(&target_dir, &metadata)?;

        fs::write(
            target_dir.join("repo/a.txt"),
            "line1
line2
line3
",
        )?;
        commit_code("Initial commit", &temp_dir.path())?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;

        let canonical_path = "//third_party/example";
        let target_dir = path_to_abs(&paths, canonical_path)?;

        fs::write(
            target_dir.join("patches/0001-update-line1.patch"),
            "diff --git a/a.txt b/a.txt
index 83db48f..efc6926 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line999
+line123
 line2
 line3
",
        )?;
        commit_code("Create patch1", &target_dir)?;

        // make all patches pending
        metadata.update_state = Some(UpdateState {
            prev_commit_hash: get_current_commit()?,
            patches: load_patch_list(&target_dir)?
                .iter()
                .map(|e| PatchApplyState {
                    name: e.clone(),
                    state: PatchState::Pending,
                })
                .collect(),
        });
        update_metadata(&target_dir, &metadata)?;

        let apply_result = apply_patches(&target_dir, canonical_path, &paths, &mut metadata);
        assert!(
            apply_result.is_err(),
            "Expected Err, but got {:?}",
            apply_result
        );

        let new_metadata = load_metadata(&target_dir)?;
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[0].state,
            PatchState::Conflict,
            "expected Conflict state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_update_apply_patch_with_conflict_and_continue() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        let target_dir = temp_dir.path().join("third_party/example");

        let mut metadata = DependencyMetadata {
            url: "empty".to_string(),
            version: "default".to_string(),
            update_state: None,
        };
        update_metadata(&target_dir, &metadata)?;

        fs::write(
            target_dir.join("repo/a.txt"),
            "line1
line2
line3
",
        )?;
        commit_code("Initial commit", &temp_dir.path())?;

        let paths = paths::MonorepoPaths::from_dir(temp_dir.path())
            .context("Could not find monorepo checkout paths")?;

        let canonical_path = "//third_party/example";
        let target_dir = path_to_abs(&paths, canonical_path)?;

        fs::write(
            target_dir.join("patches/0001-update-line1.patch"),
            "diff --git a/a.txt b/a.txt
index 83db48f..efc6926 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line999
+line123
 line2
 line3
",
        )?;
        commit_code("Create patch1", &target_dir)?;

        // make all patches pending
        metadata.update_state = Some(UpdateState {
            prev_commit_hash: get_current_commit()?,
            patches: load_patch_list(&target_dir)?
                .iter()
                .map(|e| PatchApplyState {
                    name: e.clone(),
                    state: PatchState::Pending,
                })
                .collect(),
        });
        update_metadata(&target_dir, &metadata)?;

        let apply_result = apply_patches(&target_dir, canonical_path, &paths, &mut metadata);
        assert!(
            apply_result.is_err(),
            "Expected Err, but got {:?}",
            apply_result
        );

        let new_metadata = load_metadata(&target_dir)?;
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[0].state,
            PatchState::Conflict,
            "expected Conflict state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );

        fs::write(
            target_dir.join("repo/a.txt"),
            "line333
line2
line3
",
        )?;
        apply_patches(&target_dir, canonical_path, &paths, &mut metadata)?;

        let new_metadata = load_metadata(&target_dir)?;
        assert_eq!(
            new_metadata.update_state.clone().unwrap().patches[0].state,
            PatchState::Resolved,
            "expected Resolved state for the patch, got {:?}",
            new_metadata.update_state.unwrap()
        );

        let patch_content = fs::read_to_string(target_dir.join("patches/0001-update-line1.patch"))?;
        assert_eq!(
            normalize_patch(&patch_content),
            "diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
-line1
+line333
 line2
 line3",
        );

        Ok(())
    }

    #[test]
    fn integration_vendor_and_patch_test() -> anyhow::Result<()> {
        let temp_dir = create_test_dir()?;

        fs::write(temp_dir.path().join(".keep"), "")?;
        fs::create_dir_all(temp_dir.path().join("third_party"))?;
        commit_code("Initial commit", &temp_dir.path())?;

        let paths = MonorepoPaths::from_dir(temp_dir.path())?;

        // Vendor third-party dep
        vendor(
            VendorCommandArgs {
                git: "https://github.com/khamutov/dockyard.git".to_string(),
                version: Some("879bfd9".to_string()),
                path: "//third_party/dockyard".to_string(),
            },
            &paths,
        )?;
        commit_code("Vendor dockyard", &temp_dir.path())?;

        // Edit Cargo.toml
        fs::write(
            temp_dir.path().join("third_party/dockyard/repo/Cargo.toml"),
            r#"[package]
name = "dockyard-vendored"
description = "Monorepo managment tool"
authors = ["Aleksandr Khamutov"]
version = "0.0.0"
edition = "2024"
license-file = "LICENSE"

[dependencies]
anyhow = "1.0.98"
clap = {version = "4.5.38", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
"#,
        )?;
        extract_patch(
            ExtractPatchCommandArgs {
                path: "//third_party/dockyard".to_string(),
            },
            &paths,
        )?;
        git_add_all(temp_dir.path())?;
        commit_code("Update vendored package name", temp_dir.path())?;

        // Update vendored code to new version
        update(
            UpdateCommandArgs {
                version: Some("a784ec0".to_string()),
                force: false,
                status: false,
                cont: false,
                path: Some("//third_party/dockyard".to_string()),
            },
            &paths,
        )?;

        let cargo_toml_content: Vec<String> =
            fs::read_to_string(temp_dir.path().join("third_party/dockyard/repo/Cargo.toml"))?
                .lines()
                .map(|s| s.to_string())
                .collect();

        // Check the patch was applied to new code
        assert!(cargo_toml_content.len() > 2);
        assert_eq!(cargo_toml_content[1], "name = \"dockyard-vendored\"");

        let metadata: DependencyMetadata =
            load_metadata(&temp_dir.path().join("third_party/dockyard"))?;

        // Check the version was updated in metadata
        assert_eq!(metadata.version, "a784ec0");

        Ok(())
    }

    fn create_test_dir() -> anyhow::Result<TempDir> {
        let temp_dir = tempdir()?;

        fs::create_dir_all(temp_dir.path().join("third_party"))?;

        // create expected dir structure
        fs::create_dir_all(temp_dir.path().join("third_party/example/repo"))?;
        fs::create_dir_all(temp_dir.path().join("third_party/example/patches"))?;

        init_git(temp_dir.path())?;

        Ok(temp_dir)
    }

    fn init_git(current_dir: &Path) -> anyhow::Result<()> {
        let commit_cmd = Command::new("git")
            .current_dir(current_dir)
            .args(["init"])
            .output()?;

        if !commit_cmd.status.success() {
            bail!(
                "git init failed, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&commit_cmd.stdout),
                String::from_utf8_lossy(&commit_cmd.stderr),
            );
        }

        Ok(())
    }

    // Removes for testing unstable lines (e.g. index with blob hashes) from patches
    fn normalize_patch(p: &str) -> String {
        p.lines()
            .filter(|l| !l.starts_with("index "))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
