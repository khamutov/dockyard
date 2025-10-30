use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::ExtractPatchCommandArgs;
use crate::UpdateCommandArgs;
use crate::VendorCommandArgs;
use crate::paths;
use anyhow::Context;
use anyhow::{Result, anyhow};
use dockyard::paths::path_to_abs;
use dockyard::utils::run_command;
use serde::Serialize;

#[derive(Serialize)]
struct DependencyMetadata {
    url: String,
    version: String,
}

pub fn vendor(args: VendorCommandArgs, paths: &paths::MonorepoPaths) -> Result<()> {
    let target_dir = path_to_abs(paths, &args.path)?;

    // 1. check target path doesn't exist
    if target_dir.exists() {
        return Err(anyhow!("Target must be empty: {}", target_dir.display()));
    }
    // 2. create target path
    fs::create_dir_all(&target_dir)?;

    // 3. clone the git repository to `repo` subdir
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
    // 4. remove .git from the cloned repo
    fs::remove_dir_all(clone_dir.join(".git"))?;

    // 5. create metadata file
    let meta = DependencyMetadata {
        url: args.git.to_string(),
        version: version_str.to_string(),
    };
    let json = serde_json::to_string_pretty(&meta)?;
    fs::write(target_dir.join("dep_info.json"), json)?;

    Ok(())
}

pub fn update(_args: UpdateCommandArgs, _paths: &paths::MonorepoPaths) -> Result<()> {
    unimplemented!("Update is not yet implemented")
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

    let ls = Command::new("git")
        .args([
            "ls-files",
            "--others",
            "--exclude-standard",
            repo_dir.to_str().unwrap(),
        ])
        .output()?;
    if !ls.status.success() {
        return Err(anyhow!("git ls-files failed"));
    }

    if !ls.stdout.is_empty() {
        return Err(anyhow!(
            "untracked files exist under {}",
            repo_dir.display()
        ));
    }

    let patch_cmd = Command::new("git")
        .args([
            "diff".to_string(),
            // include all files (from index and unstaged)
            "HEAD".to_string(),
            format!("--relative={}", relative_path.display()),
            "--".to_string(),
            repo_dir.display().to_string(),
        ])
        .output()?;

    if !patch_cmd.status.success() {
        return Err(anyhow!("git diff failed"));
    }

    if patch_cmd.stdout.is_empty() {
        return Err(anyhow!(
            "no changes detected in third_party: {}",
            repo_dir.display(),
        ));
    }

    Ok(patch_cmd.stdout)
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use anyhow::{Context, bail};
    use dockyard::{paths::path_to_abs, *};

    use crate::vendor::extract_diff;

    #[test]
    fn test_extract_patch() -> anyhow::Result<()> {
        let paths = paths::MonorepoPaths::from_third_party_dir("test_dir")
            .context("Could not find monorepo checkout paths")?;
        let target_dir = path_to_abs(&paths, "//test_dir/repo_extract")?;
        fs::create_dir_all(&target_dir)?;

        fs::write(target_dir.join("tesfile.txt"), "line1\nline2\n")?;

        let add_file_to_git = Command::new("git")
            .args(["add", target_dir.to_str().unwrap()])
            .output()?;
        if !add_file_to_git.status.success() {
            bail!("git add failed");
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
            bail!("git restore staged file failed");
        }
        fs::remove_dir_all(target_dir)?;

        Ok(())
    }

    #[test]
    fn test_extract_patch_error_on_untracked() -> anyhow::Result<()> {
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
