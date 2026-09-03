<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai アイコン">
</a>

# socai

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語** · [한국어](README.ko.md)

**小紅書（RedNote）リサーチに最適化されたローカル Web エージェント**

ログイン済みの Chrome に接続し、投稿・コメント・画像・動画を読み取り、コンテンツ調査、競合分析、消費者インサイトの抽出を支援します。

[公式サイト](https://socai.io/?utm_source=github&utm_medium=readme) · [ダウンロード](#デスクトップアプリ) · [クイックスタート](#クイックスタート) · [コマンド](#小紅書コマンド) · [開発ドキュメント](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#デスクトップアプリ)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="ブラウザー検索から LLM 処理、構造化された調査結果までの socai ワークフロー">
</a>

</div>

## 概要

socai は、小紅書のコンテンツを深く理解する調査タスク向けのツールです。Chrome DevTools Protocol（CDP）を通じて実際のブラウザーに接続し、既存のログイン状態を再利用します。検索、投稿の閲覧、コメントの展開、著者ページの確認を画面操作で行い、結果を構造化データとローカル成果物として保存します。

主な利用例：

- カテゴリー内で新しく生まれた話題、感情、表現の追跡
- 投稿とコメント欄から、ニーズ、不安、購買判断の言葉を抽出
- ブランド、商品、店舗、キャンペーンに対する反応の比較
- アカウントの投稿方針、人気投稿、読者反応の分析
- 画像・動画の保存、OCR、動画音声文字起こしによる証拠の補完

現在は読み取りと調査を中心に提供しており、投稿、いいね、保存、コメントなどの書き込み操作は提供していません。

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## 主な機能

| 機能 | 内容 |
| --- | --- |
| 実ブラウザー操作 | 普段使っている Chrome に接続し、ページ上の操作経路に沿ってタスクを実行します。 |
| 投稿・コメントの深読 | タイトル、本文、著者、反応数、コメント、返信を取得します。 |
| マルチモーダル理解 | 投稿画像と動画の保存、ローカル画像 OCR、動画音声文字起こしに対応します。 |
| 調査フィルター | 公開時期、投稿形式、並び順、検索範囲、距離などの画面フィルターを利用できます。 |
| 証拠と成果物の保存 | 構造化結果、メディア一覧、レポートや表などを保存し、後から確認できます。 |
| 複数の操作画面 | デスクトップアプリ、CLI、ターミナル UI が同じ Rust コアを共有します。 |
| エージェント連携 | Claude Code、Codex などが扱いやすい構造化 JSON を返します。 |

## クイックスタート

### デスクトップアプリ

コマンドラインを設定せず、自然言語で調査タスクを入力できます。macOS と Windows に対応しています。

- [macOS 版をダウンロード](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows 版をダウンロード](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

インストール後、画面の案内に従って Chrome を接続し、次のようなタスクを入力します。

> 過去 1 か月の無糖茶に関する高反応の小紅書投稿を調査し、ブランド選択で重視される点を、具体的な投稿とコメントを引用して整理してください。

既存の Chrome へ初めて接続するときは、リモートデバッグを有効にし、ブラウザーの許可を確認します。[Chrome 接続ガイド](https://socai.io/connect)を参照してください。

デスクトップアプリでは、タスク履歴と成果物を保存し、レポート、表、画像などをプレビューまたはダウンロードできます。結果を Feishu のドキュメントやグループチャットへ出力することもできます。

### コマンドライン

CLI は Claude Code、Codex などのエージェント連携や、構造化データを使うワークフローに適しています。

macOS：

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell：

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

最初の検索を実行します。

```bash
socai xhs search "初心者向けキャンプ用品" --num-notes 10 --num-comments 8 --pretty
```

利用中の環境にビルド済みバイナリがない場合や、ソースから開発するときは Cargo を利用できます。

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force
```

### ターミナル UI

CLI のインストール後、サブコマンドを付けずに `socai` を実行します。

```bash
socai
```

## 利用方法

| 方式 | 適した用途 | 開始方法 |
| --- | --- | --- |
| デスクトップアプリ | 自然言語タスク、履歴、成果物の確認とダウンロード | macOS または Windows 版をインストール |
| CLI | エージェント連携、スクリプト、構造化 JSON | `socai xhs ...` を実行 |
| ターミナル UI | ターミナルで連続タスクを手動実行 | `socai` を実行 |

## 小紅書コマンド

### 投稿を検索して深く読む

```bash
socai xhs search "コンテンツ企画" \
  --num-notes 30 \
  --num-comments 20 \
  --filter publish_time=一周内 \
  --filter sort=最多评论 \
  --download-media \
  --ocr \
  --pretty
```

`search` は検索結果を開いて本文とコメントを読み取ります。`--preview` を付けると、投稿詳細を開かずにタイトル、カバー、反応数などの概要だけを返します。

### 著者と投稿を読む

```bash
socai xhs author <author_id> --num-notes 10 --num-comments 8
```

投稿概要だけを取得する場合：

```bash
socai xhs author <author_id> --num-notes 20 --preview
```

### 指定投稿を再取得する

`search` または `author` が返した投稿 ID と `xsec_token` を利用します。

```bash
socai xhs get-notes \
  --note '<note_id>=<xsec_token>' \
  --note '<note_id>=<xsec_token>' \
  --num-comments 20
```

### 主なオプション

| オプション | 内容 |
| --- | --- |
| `--num-notes <N>` | 取得する投稿数。必要に応じてページをスクロールします。 |
| `--num-comments <N>` | 投稿ごとのコメントと返信数。`0` でコメントを省略します。 |
| `--preview` | 検索結果または著者ページの投稿概要だけを読み取ります。 |
| `--download-media` | 開いた投稿の画像と動画を保存します。 |
| `--ocr` | 投稿画像または動画カバーにローカル OCR を実行します。 |
| `--transcribe-audio` | 開いた動画を保存して音声を文字起こしします。socai agent へのログインと選択が必要です。 |
| `--filter <group=option>` | 小紅書の検索フィルター。複数回指定できます。 |
| `--pretty` | 最終 JSON を読みやすく整形します。 |

フィルター値は小紅書 Web 画面の中国語表記をそのまま使用します。

| グループ | 値 |
| --- | --- |
| `sort` | 综合, 最新, 最多点赞, 最多评论, 最多收藏 |
| `note_type` | 不限, 视频, 图文 |
| `publish_time` | 不限, 一天内, 一周内, 半年内 |
| `search_scope` | 不限, 已看过, 未看过, 已关注 |
| `distance` | 不限, 同城, 附近 |

## ブラウザーとログイン

| モード | 用途 | 動作 |
| --- | --- | --- |
| `existing` | 日常利用、既定値 | 既存 Chrome と小紅書のログインを再利用 |
| `managed` | 普段の閲覧環境と分離 | `~/.socai/chrome-profile` を使用し、初回のみログイン |
| `auto` | 接続方式の自動選択 | 独立プロファイルを試し、失敗時は既存 Chrome へ接続 |
| `remote` | クラウドブラウザーのテスト | セッション制限のある socai pro ベータ機能 |

設定は `~/.socai/config.json` に保存され、CLI とデスクトップアプリで共有されます。

独立プロファイルへ切り替える場合：

```bash
socai config set chrome.profile managed
socai stop
```

既存 Chrome に戻す場合：

```bash
socai config set chrome.profile existing
socai stop
```

クラウドブラウザーを利用する場合：

```bash
socai pro activate <invite_code>
socai config set chrome.profile remote
```

## 実行結果と成果物

各実行の結果は、既定で次の場所に保存されます。

```text
~/.socai/runs/<timestamp>_<task>/
```

構造化結果、投稿・著者データ、ダウンロードした画像や動画、OCR 結果、`media_manifest.json`、レポートや表などが含まれます。

保存先は変更できます。

```bash
socai config set runs.dir "$(pwd)/socai-runs"
```

## Douyin 検索

CLI は基本的な Douyin 検索も提供します。

```bash
socai dy search "コーヒー" --num 30
```

## 拡張と開発

新しいサイトや機能を追加する場合は、[サイト拡張ガイド](core/src/sites/creation/SKILL.md)を参照してください。ローカル開発、ビルド、リポジトリ規約は [DEVELOPMENT.md](DEVELOPMENT.md) にあります。

## コミュニティ

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 小紅書リサーチ WeChat グループ QR コード" width="280">

利用方法、調査ワークフロー、機能提案に関するフィードバックを歓迎します。

## ライセンス

socai は [Apache License 2.0](LICENSE) の下で公開されています。
