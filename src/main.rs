mod vendor;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, command};
use dockyard::paths;

#[derive(Debug, Parser)]
struct DockyardArgs {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Update vendored dependency under //third_party")]
    Update(UpdateCommandArgs),
    #[command(about = "Download third-party dependency to //third_party")]
    Vendor(VendorCommandArgs),
    #[command(about = "Extract patch for third-party dependency to //third_party/dep_name/patches")]
    ExtractPatch(ExtractPatchCommandArgs),
}

#[derive(Debug, Parser)]
struct VendorCommandArgs {
    #[arg(
        long,
        help = " \
        Git repository url to import into monorepository."
    )]
    git: String,
    #[arg(
        long,
        help = " \
        Tag or commit hash to import. If skipped then the default branch will \
        be imported."
    )]
    version: Option<String>,
    #[arg(
        long,
        help = " \
        Path where to import dependency. \
        The path must be provided in the canonical format: //third_party/dep_name"
    )]
    path: String,
}

#[derive(Debug, Parser)]
struct UpdateCommandArgs {
    #[arg(
        long,
        help = " \
        Tag or commit hash to import. If skipped then HEAD will be used."
    )]
    version: Option<String>,
    #[arg(
        long,
        help = " \
        Force update and re-apply pathces even if version is the same.",
        default_value_t = false
    )]
    force: bool,
    #[arg(
        long,
        help = " \
        Show update status.",
        default_value_t = false
    )]
    status: bool,
    #[arg(
        long = "continue",
        help = " \
        Show update status.",
        default_value_t = false
    )]
    cont: bool,
    #[arg(help = " \
        Update third party dependency under specified path. \
        The path must be provided in the canonical format: //third_party/dep_name")]
    path: Option<String>,
}

#[derive(Debug, Parser)]
struct ExtractPatchCommandArgs {
    #[arg(
        long,
        help = " \
        Extracts patch from changes in third_party code. \
        The path must be provided in the canonical format: //third_party/dep_name"
    )]
    path: String,
}

fn main() -> Result<()> {
    let args = DockyardArgs::parse();

    let paths = paths::MonorepoPaths::new().context("Could not find monorepo checkout paths")?;

    match args.command {
        Command::Update(args) => vendor::update(args, &paths),
        Command::Vendor(args) => vendor::vendor(args, &paths),
        Command::ExtractPatch(args) => vendor::extract_patch(args, &paths),
    }
}
