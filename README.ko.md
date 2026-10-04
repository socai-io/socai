<div align="center">

<h1>
  <a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo"><img src="site/public/icon-192.png" width="36" alt="socai 아이콘" align="absmiddle"></a>
  socai
</h1>

**에이전트가 소셜 플랫폼을 실제로 이해하게 합니다.**

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · **한국어**

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="샤오홍슈">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="TikTok / 더우인">
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

소셜 플랫폼은 에이전트가 들어가기 어렵습니다. 스크래퍼는 계정이 정지됩니다. 범용 컴퓨터 조작은 느리고, 비싸고, 플랫폼별 지식도 없습니다.

socai는 각 플랫폼의 깊은 온톨로지를 만들어, 에이전트가 엔티티, 상태, 흐름을 실제로 이해하게 합니다. 그리고 로그인된 Chrome을 그대로 써서 소셜 계정 상태를 유지한 채, 사람처럼 검색하고, 게시물을 열고, 댓글을 펼치고, 프로필을 읽고, 이미지를 OCR하고, 영상을 전사합니다.

[![Instagram, X, TikTok에서 뷰티 트렌드 조사와 러닝 의류 크리에이터 발굴](docs/assets/research-demo-en.gif)](https://socai.io/?utm_source=github&utm_medium=readme&utm_campaign=research-demo#demo)

| Instagram 리서치 | 샤오홍슈 리서치 |
| --- | --- |
| https://github.com/user-attachments/assets/4849e0f3-87d5-4a0d-8e0b-2a58e3d0267a | https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d |

## 지원 플랫폼

**샤오홍슈(Xiaohongshu)** — 검색, 작성자, 게시물, 댓글, 미디어, OCR, 전사.

```bash
socai xhs search "sugar-free tea" --num-notes 10 --num-comments 8
socai xhs author <author_id> --num-notes 10
socai xhs get-notes --note '<note_id>=<xsec_token>' --num-comments 8
socai xhs comment '<note_url>' --text 'Exact comment text'
```

**X** — 검색, 프로필, 게시물, 답글.

```bash
socai x search "open source" --num 10
socai x profile <handle>
socai x get-posts --post https://x.com/<handle>/status/<id> --num-comments 8
socai x reply https://x.com/<handle>/status/<id> --text 'Exact reply text'
```

**더우인(Douyin)** — 검색, 영상, 작성자, 댓글.

```bash
socai dy search "coffee" --num 20
socai dy author <author_id> --num 10
socai dy get-videos --video <url> --num-comments 8
```

**TikTok** — 검색, 영상, 프로필, 댓글.

```bash
socai tiktok search "coffee" --num 20
socai tiktok author <handle> --num 10
socai tiktok get-videos --video <url> --num-comments 8
```

**Instagram** — 검색, 프로필, 게시물, Reels, 댓글.

```bash
socai instagram search "coffee" --num 10
socai instagram profile nike
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
socai instagram search_accounts "nike"
socai instagram comment https://www.instagram.com/p/<shortcode>/ --text 'Exact comment text'
```

**LinkedIn** — 사람, 회사, 게시물, 댓글.

```bash
socai linkedin search "product designer" --type people --num 10
socai linkedin profile <id>
socai linkedin company <company>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
socai linkedin comment https://www.linkedin.com/posts/<id> --text 'Exact comment text'
```

필터, 댓글, 나머지 명령은 [사용 가이드](docs/guide.md#platform-command-reference)에 있습니다.

## 시작하기

데스크톱 앱은 macOS와 Windows를 지원합니다.

- [macOS 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

macOS CLI:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows 설치, Agent 연동, 플랫폼 명령, Chrome 설정, 산출물, 로컬 개발 절차는 [사용 및 개발 가이드](docs/guide.md)에 정리되어 있습니다.

## socai로 만든 것

[Jev Social](https://github.com/socai-io/jev-social)은 로컬 우선 데모입니다. Jev가 Instagram, TikTok, LinkedIn에 대해 범위가 정해진 socai CLI 작업을 고릅니다. 가져온 게시물 카드는 그대로 보이고, 출처가 연결된 리서치 보고서가 브라우저로 들어옵니다.

[![Jev Social demo](https://raw.githubusercontent.com/socai-io/jev-social/main/docs/jev-social.gif)](https://github.com/socai-io/jev-social)

[Agent Skill 설치](https://github.com/socai-io/jev-social/tree/v0.1.13/skills/jev-social)

## 커뮤니티

[Discord 참여](https://discord.gg/CpQdA7bwt8).

WeChat 그룹 참여:

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 소셜 미디어 리서치 WeChat 그룹 QR 코드" width="280">

socai가 유용하다면 저장소에 Star를 눌러 주세요.
