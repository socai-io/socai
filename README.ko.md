<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai 아이콘">
</a>

# socai

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · **한국어**

**소셜 미디어를 실제로 읽는 에이전트.**

스크래퍼도, 리버스 엔지니어링 API도 아닙니다. socai는 이미 로그인한 Chrome 안에서 실제 페이지를 열고, 인용할 수 있는 근거를 가져옵니다.

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="小红书">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="TikTok / 抖音">
  &nbsp;&nbsp;
  <img src="site/public/platforms/instagram.png" height="32" alt="Instagram">
  &nbsp;&nbsp;
  <img src="site/public/platforms/linkedin.svg" height="32" alt="LinkedIn">
</p>
<p><sub>小红书 · TikTok / 抖音 · Instagram · LinkedIn</sub></p>

[공식 사이트](https://socai.io/?utm_source=github&utm_medium=readme) · [다운로드](#데스크톱-앱) · [Discord](https://discord.gg/CpQdA7bwt8) · [빠른 시작](#빠른-시작) · [개발 문서](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![discord](https://img.shields.io/badge/discord-join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/CpQdA7bwt8)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#데스크톱-앱)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="브라우저 검색에서 LLM 처리와 구조화된 조사 결과까지 이어지는 socai 워크플로">
</a>

</div>

## 소개

진짜 대화는 소셜 미디어에 있습니다. 공개 API는 그걸 가리고, 스크래퍼는 계정 정지를 부릅니다. socai는 세 번째 길입니다: 로그인한 Chrome을 조사하듯 움직입니다. 검색하고, 게시물을 열고, 댓글을 펼치고, 프로필을 읽고, 이미지를 OCR하고, 영상을 전사한 뒤 근거를 남깁니다.

조사 기능은 기본적으로 읽기 전용입니다. 대상을 명시한 쓰기 명령은 사용자가 직접 실행할 때만 동작하며, 영구적인 일회성 영수증으로 자동 재전송을 방지합니다.

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## 빠른 시작

### 데스크톱 앱

명령줄 환경을 설정하지 않고 자연어로 조사 작업을 입력할 수 있습니다. macOS와 Windows를 지원합니다.

- [macOS 버전 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows 버전 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

설치 후 화면 안내에 따라 Chrome을 연결하고 다음과 같은 작업을 입력합니다.

> 小红书, 抖音, Instagram에서 무가당 차가 어떻게 이야기되는지 비교하고, 반복해서 등장하는 구매 기준을 구체적인 게시물, 영상, 댓글과 답글 인용으로 정리해 주세요.

기존 Chrome에 처음 연결할 때는 원격 디버깅을 활성화하고 브라우저 권한을 확인해야 합니다. [Chrome 연결 안내](https://socai.io/connect)를 참고하세요.

데스크톱 앱은 작업 기록과 산출물을 보관합니다. 보고서, 스프레드시트, 이미지 등을 미리 보거나 다운로드할 수 있으며, 결과를 Feishu 문서나 그룹 채팅으로 내보낼 수도 있습니다.

### 명령줄

CLI는 Claude Code, Codex 등의 에이전트 연동과 구조화된 데이터 또는 스크립트 기반 워크플로에 적합합니다.

macOS:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

구조화된 플랫폼 검색을 실행합니다.

```bash
socai xhs search "초보 캠핑 장비" --num-notes 10 --num-comments 8 --pretty
socai dy search "캠핑 장비" --num 20
socai tiktok search "camping gear" --num 20 --pretty
socai instagram search "camping gear" --num 20 --pretty
socai linkedin search "product designer" --type people --num 20 --pretty
```

하위 명령 없이 `socai`를 실행하면 같은 플랫폼을 대상으로 크로스 플랫폼 조사를 에이전트에게 요청할 수 있습니다.

현재 플랫폼에 사전 빌드된 바이너리가 없거나 소스 개발이 필요한 경우 Cargo를 사용할 수 있습니다.

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force --locked
cargo install --path asr --force --locked
```

두 번째 명령은 로컬 Whisper helper를 `socai`와 같은 위치에 설치합니다. 비유료 또는 오프라인 음성 변환에서 내장 모델을 사용할 때 필요합니다.

### 터미널 UI

CLI를 설치한 뒤 하위 명령 없이 `socai`를 실행합니다.

```bash
socai
```

## 사용 방식

| 방식 | 적합한 작업 | 시작 방법 |
| --- | --- | --- |
| 데스크톱 앱 | 자연어 작업, 작업 기록, 산출물 미리 보기와 다운로드 | macOS 또는 Windows 앱 설치 |
| CLI | 에이전트 호출, 스크립트, 구조화된 JSON | `socai xhs ...`, `socai dy ...`, `socai tiktok ...`, `socai instagram ...`, `socai linkedin ...` 실행 |
| 터미널 UI | 터미널에서 연속 작업 수동 실행 | `socai` 실행 |

## 지원 플랫폼

| 플랫폼 | 조사 기능 | 사용 방식 |
| --- | --- | --- |
| 小红书 | 검색, 작성자, 게시물, 댓글과 답글, 미디어 저장, OCR, 음성 전사 | 에이전트와 구조화된 CLI |
| 抖音 | 검색, 영상 상세, 작성자, 댓글과 답글, 미디어 산출물 | 에이전트와 구조화된 CLI |
| TikTok | 검색, 영상 상세, 작성자 프로필, 댓글과 답글, 영상 저장 | 에이전트와 구조화된 CLI |
| Instagram | 키워드 검색, 프로필, 게시물, Reels, 댓글과 답글, 영상 저장 | 에이전트와 구조화된 CLI |
| LinkedIn | 인물·회사·콘텐츠 검색, 프로필, 경력, 관계 정보, 게시물, 댓글 | 에이전트와 구조화된 CLI |

조사 명령은 플랫폼 상태를 변경하지 않습니다. 별도로 문서화된 게시·댓글 명령은 명확한 대상과 내용을 요구하고, 전송 직전에 로그인 계정과 표시된 대상을 다시 확인합니다. 전송 결과가 불확실하면 자동으로 재시도하지 않습니다.

## 플랫폼 명령어

### 小红书

#### 게시물 검색과 심층 읽기

```bash
socai xhs search "콘텐츠 기획" \
  --num-notes 30 \
  --num-comments 20 \
  --filter publish_time=一周内 \
  --filter sort=最多评论 \
  --download-media \
  --ocr \
  --pretty
```

`search`는 검색 결과를 열어 본문과 댓글을 읽습니다. `--preview`를 추가하면 게시물 상세 화면을 열지 않고 제목, 표지, 반응 수치 등의 요약만 반환합니다.

#### 작성자와 게시물 읽기

```bash
socai xhs author <author_id> --num-notes 10 --num-comments 8
```

게시물 요약만 가져오려면 다음과 같이 실행합니다.

```bash
socai xhs author <author_id> --num-notes 20 --preview
```

#### 지정 게시물 다시 읽기

`search` 또는 `author`가 반환한 게시물 ID와 `xsec_token`을 사용합니다.

```bash
socai xhs get-notes \
  --note '<note_id>=<xsec_token>' \
  --note '<note_id>=<xsec_token>' \
  --num-comments 20
```

#### 주요 옵션

| 옵션 | 설명 |
| --- | --- |
| `--num-notes <N>` | 가져올 게시물 수입니다. 필요하면 페이지를 계속 스크롤합니다. |
| `--num-comments <N>` | 게시물마다 가져올 댓글과 답글 수입니다. `0`은 댓글을 건너뜁니다. |
| `--preview` | 검색 결과 또는 작성자 페이지의 게시물 요약만 읽습니다. |
| `--download-media` | 열린 게시물의 이미지와 영상을 저장합니다. |
| `--ocr` | 게시물 이미지 또는 영상 표지에 로컬 OCR을 실행합니다. |
| `--transcribe-audio` | 열린 영상을 저장하고 음성을 전사합니다. socai agent 로그인과 선택이 필요합니다. |
| `--filter <group=option>` | 小红书 검색 필터이며 여러 번 지정할 수 있습니다. |
| `--pretty` | 최종 JSON을 읽기 쉽게 출력합니다. |

필터 값은 小红书 웹 화면의 중국어 표기를 그대로 사용합니다.

| 그룹 | 값 |
| --- | --- |
| `sort` | 综合, 最新, 最多点赞, 最多评论, 最多收藏 |
| `note_type` | 不限, 视频, 图文 |
| `publish_time` | 不限, 一天内, 一周内, 半年内 |
| `search_scope` | 不限, 已看过, 未看过, 已关注 |
| `distance` | 不限, 同城, 附近 |

### 抖音과 TikTok

```bash
socai dy search "커피" --num 30
socai tiktok search "coffee" --num 30 --pretty
```

영상 상세, 작성자, 댓글, 미디어 저장과 진단 명령은 `socai dy --help` 또는 `socai tiktok --help`에서 확인할 수 있습니다.

### Instagram

```bash
socai instagram search "coffee" --num 20 --pretty
socai instagram profile nike --num 12
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
```

프로필, 게시물 / Reels, 댓글, 진단 명령은 `socai instagram --help`에서 확인할 수 있습니다.

### LinkedIn

```bash
socai linkedin search "product designer" --type people --num 20 --pretty
socai linkedin profile https://www.linkedin.com/in/<id>/
socai linkedin history <id> --section experience
socai linkedin company <company-id>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
```

회사, 관계 정보, 게시물, 댓글, 진단 명령은 `socai linkedin --help`에서 확인할 수 있습니다.

## 브라우저와 로그인

| 모드 | 용도 | 동작 |
| --- | --- | --- |
| `existing` | 일상적인 사용, 기본값 | 기존 Chrome과 지원 플랫폼의 로그인 재사용 |
| `managed` | 평소 브라우징 환경과 분리 | `~/.socai/chrome-profile` 사용, 최초 한 번 로그인 |
| `auto` | 연결 방식 자동 선택 | 독립 프로필을 먼저 시도하고 실패하면 기존 Chrome에 연결 |
| `remote` | 호스팅 브라우저 테스트 | 세션 제한이 있는 socai pro 베타 기능 |

설정은 `~/.socai/config.json`에 저장되며 CLI와 데스크톱 앱이 같은 설정을 사용합니다.

독립 프로필로 전환:

```bash
socai config set chrome.profile managed
socai stop
```

기존 Chrome으로 복귀:

```bash
socai config set chrome.profile existing
socai stop
```

호스팅 브라우저 사용:

```bash
socai pro activate <invite_code>
socai config set chrome.profile remote
```

## 실행 결과와 산출물

각 실행 결과는 기본적으로 다음 위치에 저장됩니다.

```text
~/.socai/runs/<timestamp>_<task>/
```

구조화된 결과, 검색·게시물·작성자 데이터, 저장된 이미지와 영상, OCR 결과, `media_manifest.json`, 보고서와 스프레드시트 등의 산출물이 포함됩니다.

저장 위치는 변경할 수 있습니다.

```bash
socai config set runs.dir "$(pwd)/socai-runs"
```

## 확장과 개발

새로운 사이트나 기능을 추가하려면 [사이트 확장 안내](core/src/sites/creation/SKILL.md)를 참고하세요. 로컬 개발, 빌드, 저장소 규칙은 [DEVELOPMENT.md](DEVELOPMENT.md)에 있습니다.

## 커뮤니티

[Discord 참여](https://discord.gg/CpQdA7bwt8) · 또는 WeChat 그룹 QR:

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 소셜 미디어 리서치 WeChat 그룹 QR 코드" width="280">

## 라이선스

socai는 [Apache License 2.0](LICENSE)에 따라 배포됩니다.
