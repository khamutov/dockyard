use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::Write;
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

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
struct PatchApplyState {
    name: String,
    state: PatchState,
}

#[derive(Serialize, Deserialize, Clone)]
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
    ensure_git_clean()?;
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
        commit_code(&commit_msg)?;
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
    commit_code(&commit_message)?;

    apply_patches(&target_dir, &canonical_path, paths, &mut metadata)?;

    metadata.update_state = None;
    update_metadata(&target_dir, &metadata)?;

    let commit_msg = format!("Update metadata for {}", &canonical_path);
    commit_code(&commit_msg)?;
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
        println!("Applying patches:");
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
                                idx, patches_count, patch.name, &canonical_path,
                            );
                            commit_code(&commit_msg)?;
                        }
                        Err(_) => {
                            update_state_mut.patches[idx].state = PatchState::Conflict;
                            metadata.update_state = Some(update_state_mut.clone());
                            update_metadata(target_dir, metadata)?;

                            print!(
                                "Patch cannot be applied, fix conflicts and run\n\n:\tdockyard update --continue {}\n",
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
                    commit_code(&commit_msg)?;
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
        println!("Patch applied successfully");
        println!("{}", String::from_utf8_lossy(&output.stdout));
        Ok(())
    } else {
        eprintln!("Patch failed");
        bail!("Patch failed {}", String::from_utf8_lossy(&output.stderr));
    }
}

fn commit_code(message: &str) -> Result<()> {
    let commit_cmd = Command::new("git")
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

fn ensure_git_clean() -> Result<()> {
    let git_cmd = Command::new("git")
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
    use std::{
        fs,
        process::Command,
        sync::{Mutex, OnceLock},
    };

    use anyhow::{Context, bail};
    use dockyard::{paths::path_to_abs, *};

    use crate::vendor::extract_diff;

    static GIT_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_lock() -> &'static Mutex<()> {
        GIT_TEST_MUTEX.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_extract_patch() -> anyhow::Result<()> {
        let _guard = test_lock().lock().unwrap();

        let paths = paths::MonorepoPaths::from_third_party_dir("test_dir")
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//test_dir/repo_extract")?;
        fs::create_dir_all(&target_dir)?;

        fs::write(target_dir.join("tesfile.txt"), "line1\nline2\n")?;

        let add_file_to_git = Command::new("git")
            .args(["add", target_dir.to_str().unwrap()])
            .output()?;
        if !add_file_to_git.status.success() {
            bail!(
                "git add faile, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&add_file_to_git.stdout),
                String::from_utf8_lossy(&add_file_to_git.stderr),
            );
        }

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

        let remove_files_from_git = Command::new("git")
            .args([
                "restore",
                "--staged",
                target_dir.join("tesfile.txt").to_str().unwrap(),
            ])
            .output()?;
        if !remove_files_from_git.status.success() {
            bail!(
                "git restore staged file failed, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&remove_files_from_git.stdout),
                String::from_utf8_lossy(&remove_files_from_git.stderr),
            );
        }
        fs::remove_dir_all(target_dir)?;

        Ok(())
    }

    #[test]
    fn test_extract_patch_with_repo_subdir() -> anyhow::Result<()> {
        // Git expects forward slashes ('/') as the path separator. So let's check that the
        // functions works properly with possible backslashes after PahtBuf::join as well.
        let _guard = test_lock().lock().unwrap();

        let paths = paths::MonorepoPaths::from_third_party_dir("test_dir")
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//test_dir/repo_extract_backslash")?;
        let repo_dir = target_dir.join("repo");
        fs::create_dir_all(&repo_dir)?;

        fs::write(repo_dir.join("tesfile.txt"), "line1\nline2\n")?;

        let add_file_to_git = Command::new("git")
            .args(["add", target_dir.to_str().unwrap()])
            .output()?;
        if !add_file_to_git.status.success() {
            bail!(
                "git add faile, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&add_file_to_git.stdout),
                String::from_utf8_lossy(&add_file_to_git.stderr),
            );
        }

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

        let remove_files_from_git = Command::new("git")
            .args([
                "restore",
                "--staged",
                repo_dir.join("tesfile.txt").to_str().unwrap(),
            ])
            .output()?;
        if !remove_files_from_git.status.success() {
            bail!(
                "git restore staged file failed, stdout: {}, stderr: {}",
                String::from_utf8_lossy(&remove_files_from_git.stdout),
                String::from_utf8_lossy(&remove_files_from_git.stderr),
            );
        }
        fs::remove_dir_all(target_dir)?;

        Ok(())
    }

    #[test]
    fn test_extract_patch_error_on_untracked() -> anyhow::Result<()> {
        let _guard = test_lock().lock().unwrap();

        let paths = paths::MonorepoPaths::from_third_party_dir("test_dir")
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//test_dir/repo_untracked")?;
        fs::create_dir_all(&target_dir)?;

        fs::write(target_dir.join("tesfile.txt"), "line1\nline2\n")?;

        if let Err(_) = extract_diff(&target_dir, &paths) {
            fs::remove_dir_all(target_dir)?;
            Ok(())
        } else {
            bail!("expected to produce error with untracked files")
        }
    }
}
