---
name: dy-video-transcript
description: 抖音 视频逐字稿. Transcribe actual video audio, preserve genuine time segments and distinguish captions from descriptions.
---

# 抖音视频逐字稿

## 使用与范围

用户要求抖音视频逐字稿时读取本 skill。用户的原始目标、平台、数量、预算和权限优先；这份方法不证明依赖工具、登录、下载、转写、调度或发送能力已经可用。先检查本次工具 schema、真实页面状态和输入身份，再执行。

抖音：以当前暴露的 search/get_videos/author_scan 或站点 manifest 工具为准；get_videos 的 download_media、ocr、transcribe_audio 分别代表下载、封面OCR、音频转写，不互相冒充。封面OCR不等于图文作品全部配图OCR；未暴露完整图文提取能力时必须明确缺口。话题/图文支持必须从真实页面与 schema 确认，不能沿用小红书 selector。

## 操作方法

同会话已有视频原件时，先用 read_conversation_context 读取准确的 evidence 来源编号。若本轮暴露 transcribe_source_media，用其实际 schema 传入 source_run_id、source_sequence、note_id、media_index，直接转写归档原件；不为了转写已保存的视频重新 get_videos、下载或要求平台登录。只在没有所需归档时才进行新的站点采集。工具返回的原文附件可直接交付，文字预览截断时读完整附件，不用模型补写缺失口播。

确认真实视频身份与音频来源，优先读取可得的真实字幕；否则使用本平台实际支持的转写能力。用户明确要求逐字稿时仅在原请求范围内调用服务，沿用现有计费与授权，不暴露密钥。保留原始音频/视频关联、语言、真实转写分段和时间范围；未返回时间戳时只交付无时间戳原文，不能按字数伪造秒点。无音轨、静音、权限、下载/转码/ASR失败与空结果分别报告。不要把简介、标题、OCR字幕截图或模型概述冒充完整口播逐字稿。用户要精读时，原文与清洗/结构分析分开。

## 交付与完成条件

真实逐字稿、服务实际返回的时间分段、来源视频与可用文件、音频覆盖和缺失原因。

开始、失败或部分完成都要在当前任务中保留实际步骤和来源。正文、评论、图片、音频、文件或监控的缺失不能用示意数据、推测内容、接口200或构建成功代替。遇到登录、验证码或风控交由用户正常接管，不自动绕过，不重复提交未知结果的写操作。跨任务学习不存放用户资料或平台原文。
