# 输出结构与错误处理

## stdout 是裁剪过的摘要，不是全量

socai 故意把 stdout 的结果压成紧凑版。一条笔记在 stdout 里只有这些字段：

```
note_id, url, title, author, author_id, content, date,
likes, favorites, comments_count, top_comments (精简), ocr_text
```

被裁掉的字段列在结果里 `artifact.extra_note_properties` 中，包括：

```
hashtags, images (index, url, ocr_text, ocr_ms), image_count, video,
type, author_url, location, content_source,
top_comments 的完整对象（text, author, likes, time, sub_comments[]）
```

完整未裁剪的数据在 `artifact.path` 指向的文件里，按 `note_id` 查。**实测单篇完整记录可达 60 KB 以上**，所以：

- 不要用 `--pretty` 直接把结果摊在终端里；
- 不要 Read 整个 artifact 文件；
- 需要某篇的完整字段时，用 Grep 在该文件里定位，或用脚本抽取。

## 顶层结构

### search（非 preview）

```json
{
  "notes": [ { "entity": { "note_id": "", "url": "", "title": "", "author": "",
               "author_id": "", "content": "", "date": "", "likes": "",
               "favorites": "", "comments_count": "", "top_comments": [],
               "ocr_text": [] } } ],
  "artifact": { "path": "", "note": "", "extra_note_properties": [] },
  "media_manifest_path": "", "media_manifest_count": 0,
  "count": 0, "query": "", "url": ""
}
```

### search --preview

```json
{
  "cards": [ { "note_id": "", "title": "", "author": "", "author_id": "",
               "likes": "", "type": "", "url": "", "ocr_text": "" } ],
  "artifact": { "path": "", "note": "", "extra_note_properties": [] },
  "count": 0, "query": "", "url": ""
}
```

### author

```json
{
  "profile": {
    "entity_type": "author", "display_name": "", "title": "", "xhs_id": "",
    "url": "", "avatar_url": "", "bio": "", "ip_location": "",
    "verified": true, "verification": "企业认证",
    "followers": "", "following": "", "likes_and_collections": "",
    "note_count": 0,
    "note_cards": [ { "note_id": "", "title": "", "author": "", "author_id": "",
                      "author_url": "", "likes": "", "link": "",
                      "cover_url": "", "type": "", "position": 0,
                      "xsec_token": "" } ]
  },
  "notes": [ { "entity": { } } ],
  "author_id": "",
  "artifact": { "path": "", "note": "", "extra_note_properties": [] }
}
```

`verified` 与 `verification` 只对认证账号出现，普通账号没有这两个字段。

### get-notes

同 search 非 preview，`notes[].entity` 结构一致，另有 `media_manifest_path` 与
`media_manifest_count`。

## 字段读法

| 字段 | 说明 |
|---|---|
| `date` | 归一化日期，形如 `2026-9-27` 或 `9-27`（当年）。`date_edited: true` 表示这是最后编辑日期而非首发日期 |
| `likes` / `favorites` / `comments_count` | **字符串**，页面原文，可能是 `1.2万`、`999+`。做数值比较前先换算（万=×10000，k=×1000） |
| `top_comments[]` | stdout 里是精简版；`replies` / `sub_comments` 计入 `--num-comments` 的额度 |
| `ocr_text` | 数组，按图片顺序，封面在前 |
| `location` | 笔记挂载的 POI 地点，与作者 IP 属地 `ip_location` 是两回事（该字段在 artifact 里） |
| `url` | **唯一可引用的链接**，含 `xsec_token` |

## 互动数字换算

小红书把数字渲染成中文单位，脚本处理前必须换算：

| 原文 | 数值 |
|---|---|
| `1.2万` | 12000 |
| `3.5w` | 35000 |
| `1.5k` | 1500 |
| `999+` | 999 |
| `1,234` | 1234 |
| `收藏`（无数字） | 页面隐藏了计数，当作"未知"，**不要当 0** |

## 错误与阻断

结果里的 `reason` 字段决定下一步动作：

| reason | 含义 | 正确处理 |
|---|---|---|
| `login_required` | 未登录小红书 | **不要重试**。提示用户在浏览器里扫码登录，登录好后再跑一次 |
| `remote_browser: true` | 跑在 socai 托管云端浏览器上 | 登录由 socai 侧控制。告知用户该服务暂不可用、稍后再试，**不要让用户扫码** |
| `rate_limited` | 触发平台限流 | 换更窄的查询或等待后重试一次，不要同参数反复重试 |
| `challenge_required` | 安全验证 / 验证码 | 平台级阻断，用已有证据作答并说明缺口 |
| `commit_unknown` | 写操作结果不确定 | **不是重试许可**。如实告知，重新读一次笔记再决定 |

页面文案 `帖子不见了` / `内容无法展示` / 404 / 空白详情，都属于内容级阻断：
不要循环重试同一条命令，换路径或如实说明。

## CLI 层面的状态检查

`socai status --json` 的 `error_code` 与 `next_step`：

| error_code | next_step |
|---|---|
| `BROWSER_NOT_CONNECTED` | 需要浏览器时跑一条只读平台命令即可 |
| `BROWSER_DISCONNECTED` | 从 socai 重新连接后再开始 |
| `BROWSER_PERMISSION_REQUIRED` | 允许 Chrome 数据访问与远程调试后重试一次 |
| `BROWSER_ENDPOINT_UNREACHABLE` | 启动受支持的 Chrome，或切到托管档案 |
| `REMOTE_SESSION_UNAVAILABLE` | 检查 socai pro 与托管浏览器服务，稍后重试 |
| `DAEMON_UNAVAILABLE` | 先跑一条只读平台命令，再重试 status |

首次连接 Chrome 会弹系统权限确认框，**必须由用户手动点击**，脚本无法代劳。
