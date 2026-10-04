---
title: Development
description: Build the Rust core, CLI, desktop app, and documentation site from the socai monorepo.
---

The repository contains a Rust core, Rust CLI and terminal interface, a Tauri desktop app, and the Astro website that serves these docs.

## Clone and inspect the project

```bash
git clone https://github.com/socai-io/socai.git
cd socai
```

Read [`AGENTS.md`](https://github.com/socai-io/socai/blob/main/AGENTS.md) before changing implementation code and [the guide’s Local development section](https://github.com/socai-io/socai/blob/main/docs/guide.md#local-development) for the maintained local workflows.

## Rust workspace

```bash
cargo build
cargo test
cargo run -p socai-cli -- xhs search "运营爆款思路" --num-notes 10
```

## Desktop app

```bash
cd app
pnpm install
pnpm exec tauri dev
```

The frontend is Vite and vanilla TypeScript; the shell is Tauri 2.

## Website and docs

```bash
cd site
pnpm install
pnpm dev
pnpm build
```

The marketing pages and Starlight documentation build together into `site/dist/`. Documentation sources live under `site/src/content/docs/docs/`, which maps them to `socai.io/docs/*`.

## Contribute documentation

Each docs page includes an **Edit page** link to its source file on GitHub. Keep user-facing guides focused on supported behavior and run the full site build before opening a pull request.
