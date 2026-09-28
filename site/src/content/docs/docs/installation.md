---
title: Installation
description: Install the socai desktop app or command-line interface on macOS and Windows.
sidebar:
  order: 3
---

## Desktop app

The desktop app includes a task composer, run history, and artifact previews.

- [Download for macOS](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Download for Windows](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

After installation, open the app and follow the Chrome connection instructions.

## Command line

### macOS

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

### Windows PowerShell

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'
Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer
Unblock-File $installer
& $installer
```

The installers verify the release archive and configure, or explain, the required PATH update.

## Verify the installation

```bash
socai --version
socai --help
```

Run `socai` without a subcommand to open the terminal interface.

For platform CLI operations, start every new task with
`socai task begin "<original user question>"`, then run the platform commands.
See the [CLI workflow](/docs/cli/) for the full sequence.

## Build from source

Use a source build when no release binary is available for your platform or when you are developing socai:

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force --locked
cargo install --path asr --force --locked
```

The second command installs the local Whisper helper used for unpaid or offline transcription routes.
