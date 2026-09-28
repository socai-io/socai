# 任务上下文登记

每个新任务开始时调用一次，传用户原始问题：

```bash
socai task begin "帮我研究无糖茶消费者为什么复购，给新品定位一些建议。" --agent-host workbuddy
socai xhs search "无糖茶 复购"
socai xhs search "无糖茶 不回购"
```

后续站点命令自动归入 daemon 的当前任务，不需要传 ID。新需求再次调用 `task begin`；
同一任务内换关键词、深挖、重试不重复登记。daemon 重启后需重新登记。
多个 agent 共用一个 daemon 时，共享最近一次登记的边界。

保留用户原话，不要改成摘要或搜索关键词。长文本、换行或含 shell 特殊字符时，
用文件工具或 JSON 序列化器创建本次任务独立的 UTF-8 JSON 文件，避免把用户文本拼接进 shell：

```json
{
  "user_prompt": "帮我研究无糖茶消费者为什么复购，给新品定位一些建议。",
  "agent_host": "workbuddy"
}
```

```bash
socai task begin --context-file /absolute/task-specific-dir/context.json
```

也支持 `--context-file -` 从 stdin 读取 JSON。文件登记后可删除。只传原始问题与可选的
宿主标识；原话不在上下文中时填 `null`，不要扫描其他会话找补。`agent_host` 按实际宿主
填写，不确定时省略，默认为 `unknown`。不要传其他对话、系统提示词、附件全文或凭证。

登记不会连接 Chrome。返回 JSON 内的 ID 供分析使用，后续命令不需要它。
`socai task begin --help` 可检查支持情况；旧 CLI 不支持时继续正常研究，不自动升级或
反复重试。登记失败不会建立新边界，也不会清除 daemon 原有任务。
