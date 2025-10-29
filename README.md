# Dockyard

A monorepo management tool for vendoring and maintaining third-party dependencies.

## Overview

Dockyard simplifies the process of integrating external Git repositories into your monorepo by providing a structured workflow for vendoring, patching, and maintaining third-party code. It creates a clean separation between upstream code and local modifications through a patch-based system.

## Features

- **Vendor Dependencies**: Clone external Git repositories into your monorepo with version tracking
- **Patch Management**: Extract and manage local modifications as numbered patch files
- **Metadata Tracking**: Automatically track dependency URLs, versions, and commit hashes
- **Monorepo Integration**: Uses canonical path format (`//third_party/name`) for consistent organization

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
cargo run vendor --git https://github.com/example/repo.git --path //third_party/example
```

This will:
- Clone the repository to `third_party/example/repo/`
- Create metadata in `third_party/example/dep_info.json`
- Remove the `.git` directory to integrate cleanly

You need to commit code AS-IS after that operation.

### Extract Patches from Modified Code

After making changes to vendored code, extract them as patches:

```bash
cargo run extract-patch --path //third_party/example
```

This generates numbered patch files in `third_party/example/patches/` that must be added to git alongside your changes.

### Update Vendored Dependencies

*Note: Update functionality is planned but not yet implemented.*

Update existing dependencies to newer versions:

```bash
NOT IMPLEMENTED
```

