# 连接排查

先用 `command -v socai`（Windows 用 `Get-Command socai`）、`socai version --no-check --json` 和 `socai status --json` 确认实际程序与状态。不自动更新安装。

检查当前 `SOCAI_HOME`（默认用户目录下 `.socai`）中的 `rust-daemon.pid` 和 `rust-daemon.log` 最近记录及修改时间。macOS/Linux 核对 PID 和 `rust-daemon.sock` 的监听进程；Windows 核对 `rust-daemon-endpoint.json` 中的端口和监听进程，不输出 token。文件存在不代表进程仍活着，旧日志也不能说明当前状态。

本地 Chrome 需要核对所选用户数据目录的 `DevToolsActivePort` 第一行端口及监听进程，不输出 WebSocket 路径。部分 Chrome 的 `/json/version` 会返回 404，不能单凭它判断连接失败。已有 CDP 连接时先确认持有服务，不创建重复连接或停止未知进程；Chrome 的授权提示由用户手动允许。

超时后先检查原命令是否仍在运行、是否已有结果，再决定下一步。保留现有 Chrome、标签和 profile，不抢前台，不自动重发、停止 daemon 或切换浏览器。确需停止本任务的空闲 daemon 或切换 profile 时，先说明影响并取得用户确认，再只重试一次只读查询。

只报告实际观察到的状态；不要输出凭证、用户原文或带令牌的 URL，也不要把另一台电脑的正常结果当作本次恢复成功。
