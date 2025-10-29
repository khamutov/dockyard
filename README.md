<p align="center">
  <img src="https://raw.githubusercontent.com/khamutov/dockyard/refs/heads/main/assets/dockyard.svg" style="max-height:100%;" height="175">
</p>

# Dockyard

A monorepo management tool for vendoring and maintaining third-party dependencies.

## Overview

Dockyard simplifies the process of integrating external Git repositories into your monorepo by providing a structured workflow for vendoring, patching, and maintaining third-party code. It creates a clean separation between upstream code and local modifications through a patch-based system.

## Features

- **Vendor Dependencies**: Clone external Git repositories into your monorepo with version tracking
- **Patch Management**: Extract and manage local modifications as numbered patch files
- **Metadata Tracking**: Automatically track dependency URLs, versions, and commit hashes
- **Monorepo Integration**: Uses canonical path format (`//third_party/name`) for consistent organization

## Installation

Install Dockyard directly from the Git repository using Cargo:

```bash
cargo install --git https://github.com/khamutov/dockyard.git
```

This will build and install the `dockyard` binary to your Cargo bin directory (typically `~/.cargo/bin/`).

### Local Development

For development or testing without installing globally, you can also run directly from the source:

```bash
git clone https://github.com/khamutov/dockyard.git
cd dockyard
cargo build --release
# Run using: cargo run -- <command> <args>
```

## Vendored Code Structure

```
third_party/
└── example/
    ├── repo/           # Vendored source code
    ├── patches/        # Local modifications as patch files
    └── dep_info.json   # Dependency metadata
```

## Usage

### Vendor a New Dependency

Import an external Git repository into your monorepo:

```bash
dockyard vendor --git https://github.com/example/repo.git --path //third_party/example
```

This will:
- Clone the repository to `third_party/example/repo/`
- Create metadata in `third_party/example/dep_info.json`
- Remove the `.git` directory to integrate cleanly

You need to commit code AS-IS after that operation.

### Extract Patches from Modified Code

After making changes to vendored code, extract them as patches:

```bash
dockyard extract-patch --path //third_party/example
```

This generates numbered patch files in `third_party/example/patches/` that must be added to git alongside your changes.

### Update Vendored Dependencies

*Note: Update functionality is planned but not yet implemented.*

Update existing dependencies to newer versions:

```bash
NOT IMPLEMENTED
```

