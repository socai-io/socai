<div align="center">

<h1>
  <a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo"><img src="site/public/icon-192.png" width="36" alt="socai アイコン" align="absmiddle"></a>
  socai
</h1>

**エージェントに、ソーシャルプラットフォームを本当に理解させる。**

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語** · [한국어](README.ko.md)

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="小紅書">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="TikTok / 抖音">
  &nbsp;&nbsp;
  <img src="site/public/platforms/instagram.png" height="32" alt="Instagram">
  &nbsp;&nbsp;
  <img src="site/public/platforms/linkedin.svg" height="32" alt="LinkedIn">
  &nbsp;&nbsp;
  <img src="site/public/platforms/x.png" height="32" alt="X">
</p>

[![website](https://img.shields.io/badge/website-socai.io-555?style=flat-square&color=blue)](https://socai.io/?utm_source=github&utm_medium=readme)
[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![discord](https://img.shields.io/badge/discord-join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/CpQdA7bwt8)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

</div>

ソーシャルプラットフォームは、エージェントにとって入りにくい。スクレイパーは凍結されやすい。汎用のコンピュータ操作は遅く、高く、各プラットフォームの知見もない。

socai は各プラットフォームの深いオントロジーを組み立て、エージェントがエンティティ、状態、フローを実際に理解できるようにする。そのうえでログイン済みの Chrome をそのまま使い、ソーシャル上の状態を保ったまま、人と同じように検索し、投稿を開き、コメントを展開し、プロフィールを読み、画像を OCR し、動画を文字起こしする。

[![Instagram・X・TikTokで美容トレンドを調査し、ランニングウェアのクリエイターを発見](docs/assets/research-demo-en.gif)](https://socai.io/?utm_source=github&utm_medium=readme&utm_campaign=research-demo#demo)

| Instagram リサーチ | 小紅書リサーチ |
| --- | --- |
| https://github.com/user-attachments/assets/4849e0f3-87d5-4a0d-8e0b-2a58e3d0267a | https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d |

## 対応プラットフォーム

**小紅書（Xiaohongshu）** — 検索、著者、投稿、コメント、メディア、OCR、文字起こし。

```bash
socai xhs search "sugar-free tea" --num-notes 10 --num-comments 8
socai xhs author <author_id> --num-notes 10
socai xhs get-notes --note '<note_id>=<xsec_token>' --num-comments 8
socai xhs comment '<note_url>' --text 'Exact comment text'
```

**X** — 検索、プロフィール、投稿、返信。

```bash
socai x search "open source" --num 10
socai x profile <handle>
socai x get-posts --post https://x.com/<handle>/status/<id> --num-comments 8
socai x reply https://x.com/<handle>/status/<id> --text 'Exact reply text'
```

**抖音（Douyin）** — 検索、動画、著者、コメント。

```bash
socai dy search "coffee" --num 20
socai dy author <author_id> --num 10
socai dy get-videos --video <url> --num-comments 8
```

**TikTok** — 検索、動画、プロフィール、コメント。

```bash
socai tiktok search "coffee" --num 20
socai tiktok author <handle> --num 10
socai tiktok get-videos --video <url> --num-comments 8
```

**Instagram** — 検索、プロフィール、投稿、Reels、コメント。

```bash
socai instagram search "coffee" --num 10
socai instagram profile nike
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
socai instagram search_accounts "nike"
socai instagram comment https://www.instagram.com/p/<shortcode>/ --text 'Exact comment text'
```

**LinkedIn** — 人物、会社、投稿、コメント。

```bash
socai linkedin search "product designer" --type people --num 10
socai linkedin profile <id>
socai linkedin company <company>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
socai linkedin comment https://www.linkedin.com/posts/<id> --text 'Exact comment text'
```

フィルター、コメント、その他のコマンドは[ユーザーガイド](docs/guide.md#platform-command-reference)にあります。

## はじめる

デスクトップアプリは macOS と Windows に対応しています。

- [macOS をダウンロード](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows をダウンロード](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

macOS の CLI：

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows のインストール、Agent 連携、プラットフォームコマンド、Chrome の設定、成果物、ローカル開発の手順は[利用・開発ガイド](docs/guide.md)にまとめています。

## socai で作ったもの

[Jev Social](https://github.com/socai-io/jev-social) はローカル優先のデモです。Jev が Instagram、TikTok、LinkedIn 向けに範囲を限った socai CLI 操作を選びます。取得した投稿カードは表示されたまま、出典付きの調査レポートがブラウザに流れ込みます。

[![Jev Social demo](https://raw.githubusercontent.com/socai-io/jev-social/main/docs/jev-social.gif)](https://github.com/socai-io/jev-social)

[Agent Skill をインストール](https://github.com/socai-io/jev-social/tree/v0.1.13/skills/jev-social)

## コミュニティ

[Discord に参加](https://discord.gg/CpQdA7bwt8)。

WeChat グループに参加：

<img src="docs/assets/wechat-group-qr.jpg" alt="socai ソーシャルメディア調査 WeChat グループ QR コード" width="280">

socai が役に立ったら、リポジトリに Star を付けてください。
