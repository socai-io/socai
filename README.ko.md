<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai 아이콘">
</a>

# socai

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · **한국어**

**샤오홍슈(레드노트) 리서치에 최적화된 로컬 웹 에이전트**

로그인된 Chrome에 연결해 게시물, 댓글, 이미지와 영상을 읽고 콘텐츠 조사, 경쟁 분석, 소비자 인사이트 도출을 지원합니다.

[공식 사이트](https://socai.io/?utm_source=github&utm_medium=readme) · [다운로드](#데스크톱-앱) · [빠른-시작](#빠른-시작) · [명령어](#샤오홍슈-명령어) · [개발 문서](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#데스크톱-앱)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="브라우저 검색에서 LLM 처리와 구조화된 조사 결과까지 이어지는 socai 워크플로">
</a>

</div>

## 소개

socai는 샤오홍슈 콘텐츠를 깊이 이해해야 하는 리서치 작업을 위해 만들어졌습니다. Chrome DevTools Protocol(CDP)을 통해 실제 브라우저에 연결하고 기존 로그인 상태를 재사용합니다. 페이지에서 검색하고, 게시물을 열고, 댓글을 펼치고, 작성자 페이지를 확인한 뒤 결과를 구조화된 데이터와 로컬 산출물로 저장합니다.

주요 활용 사례:

- 특정 카테고리에서 새롭게 나타나는 주제, 감정, 표현 방식 추적
- 게시물과 댓글에서 소비자 요구, 우려, 구매 결정 언어 도출
- 브랜드, 제품, 매장 또는 캠페인에 대한 반응 비교
- 계정의 콘텐츠 방향, 인기 게시물, 독자 반응 분석
- 이미지와 영상을 저장하고 OCR 및 영상 음성 전사로 근거 보완

현재 제품은 읽기와 조사에 초점을 두며 게시, 좋아요, 저장, 댓글 작성 기능은 제공하지 않습니다.

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## 주요 기능

| 기능 | 설명 |
| --- | --- |
| 실제 브라우저 실행 | 평소 사용하는 Chrome에 연결해 페이지 상호작용 경로를 따라 작업합니다. |
| 게시물과 댓글 심층 읽기 | 제목, 본문, 작성자, 반응 수치, 댓글과 답글을 수집합니다. |
| 멀티모달 이해 | 게시물 이미지와 영상 저장, 로컬 이미지 OCR, 영상 음성 전사를 지원합니다. |
| 조사 필터와 표본 수집 | 게시 시점, 게시물 유형, 정렬, 검색 범위, 거리 등의 화면 필터를 사용합니다. |
| 근거와 산출물 보관 | 구조화된 결과, 미디어 목록, 보고서와 표 등의 산출물을 저장합니다. |
| 여러 사용 방식 | 데스크톱 앱, CLI, 터미널 UI가 동일한 Rust 코어를 공유합니다. |
| 에이전트 연동 | Claude Code, Codex 등에서 바로 사용할 수 있는 구조화된 JSON을 반환합니다. |

## 빠른 시작

### 데스크톱 앱

명령줄 환경을 설정하지 않고 자연어로 조사 작업을 입력할 수 있습니다. macOS와 Windows를 지원합니다.

- [macOS 버전 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows 버전 다운로드](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

설치 후 화면 안내에 따라 Chrome을 연결하고 다음과 같은 작업을 입력합니다.

> 최근 한 달간 무가당 차에 관한 반응이 높은 샤오홍슈 게시물을 조사하고, 사용자가 브랜드를 선택할 때 중요하게 보는 요소를 구체적인 게시물과 댓글 인용과 함께 정리해 주세요.

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

첫 번째 검색을 실행합니다.

```bash
socai xhs search "초보 캠핑 장비" --num-notes 10 --num-comments 8 --pretty
```

현재 플랫폼에 사전 빌드된 바이너리가 없거나 소스 개발이 필요한 경우 Cargo를 사용할 수 있습니다.

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force
cargo install --path asr --force
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
| CLI | 에이전트 호출, 스크립트, 구조화된 JSON | `socai xhs ...` 실행 |
| 터미널 UI | 터미널에서 연속 작업 수동 실행 | `socai` 실행 |

## 샤오홍슈 명령어

### 게시물 검색과 심층 읽기

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

### 작성자와 게시물 읽기

```bash
socai xhs author <author_id> --num-notes 10 --num-comments 8
```

게시물 요약만 가져오려면 다음과 같이 실행합니다.

```bash
socai xhs author <author_id> --num-notes 20 --preview
```

### 지정 게시물 다시 읽기

`search` 또는 `author`가 반환한 게시물 ID와 `xsec_token`을 사용합니다.

```bash
socai xhs get-notes \
  --note '<note_id>=<xsec_token>' \
  --note '<note_id>=<xsec_token>' \
  --num-comments 20
```

### 주요 옵션

| 옵션 | 설명 |
| --- | --- |
| `--num-notes <N>` | 가져올 게시물 수입니다. 필요하면 페이지를 계속 스크롤합니다. |
| `--num-comments <N>` | 게시물마다 가져올 댓글과 답글 수입니다. `0`은 댓글을 건너뜁니다. |
| `--preview` | 검색 결과 또는 작성자 페이지의 게시물 요약만 읽습니다. |
| `--download-media` | 열린 게시물의 이미지와 영상을 저장합니다. |
| `--ocr` | 게시물 이미지 또는 영상 표지에 로컬 OCR을 실행합니다. |
| `--transcribe-audio` | 열린 영상을 저장하고 음성을 전사합니다. socai agent 로그인과 선택이 필요합니다. |
| `--filter <group=option>` | 샤오홍슈 검색 필터이며 여러 번 지정할 수 있습니다. |
| `--pretty` | 최종 JSON을 읽기 쉽게 출력합니다. |

필터 값은 샤오홍슈 웹 화면의 중국어 표기를 그대로 사용합니다.

| 그룹 | 값 |
| --- | --- |
| `sort` | 综合, 最新, 最多点赞, 最多评论, 最多收藏 |
| `note_type` | 不限, 视频, 图文 |
| `publish_time` | 不限, 一天内, 一周内, 半年内 |
| `search_scope` | 不限, 已看过, 未看过, 已关注 |
| `distance` | 不限, 同城, 附近 |

## 브라우저와 로그인

| 모드 | 용도 | 동작 |
| --- | --- | --- |
| `existing` | 일상적인 사용, 기본값 | 기존 Chrome과 샤오홍슈 로그인 재사용 |
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

## Douyin 검색

CLI는 기본적인 Douyin 검색도 제공합니다.

```bash
socai dy search "커피" --num 30
```

## 확장과 개발

새로운 사이트나 기능을 추가하려면 [사이트 확장 안내](core/src/sites/creation/SKILL.md)를 참고하세요. 로컬 개발, 빌드, 저장소 규칙은 [DEVELOPMENT.md](DEVELOPMENT.md)에 있습니다.

## 커뮤니티

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 샤오홍슈 리서치 WeChat 그룹 QR 코드" width="280">

사용 경험, 조사 워크플로, 기능 제안에 대한 피드백을 환영합니다.

## 라이선스

socai는 [Apache License 2.0](LICENSE)에 따라 배포됩니다.
