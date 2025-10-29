use std::fs;
use std::fs::File;
use std::io::Write;
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

    let relative_path = repo_dir.strip_prefix(&paths.root)?;
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
    file.write_all(&patch_cmd.stdout)?;

    println!("Patch written to: {}", patch_path.display());

    Ok(())
}
