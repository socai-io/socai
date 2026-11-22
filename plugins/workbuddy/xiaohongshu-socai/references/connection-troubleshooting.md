# 连接排查

连接超时不代表后台任务已经停止。先确认发生问题的那台电脑，不要拿另一台机器的正常状态代替验收。

1. 确认实际执行的程序和版本：macOS/Linux 用 `command -v socai`，Windows 用 `Get-Command socai`，再运行 `socai version --no-check --json` 和 `socai status --json`。不要在排障时自动升级或覆盖安装。
2. 检查当前 `SOCAI_HOME`，未设置时是用户目录下的 `.socai`。读取该目录的 `rust-daemon.pid`、`rust-daemon.log` 的最近记录，先核对修改时间。旧日志只能证明以前发生过问题。不要公开用户原文、完整 URL、令牌或配置凭证。
3. macOS/Linux 核对 PID 是否仍在运行，以及 `rust-daemon.sock` 是否确有进程监听；Windows 读取 `rust-daemon-endpoint.json` 中的端口并核对监听进程，不输出其中的 token。文件存在不等于服务可用；也不要删除其他进程正在使用的 socket。
4. 使用本地 Chrome 时，核对所选浏览器用户数据目录的 `DevToolsActivePort` 第一行端口是否仍由 Chrome 监听，以及是否已有其他 CDP 客户端占用。不要输出第二行 WebSocket 路径。部分 Chrome 的 `/json/version` 返回 404，但 WebSocket 仍可用，不能据此判断 Chrome 已坏。
5. Chrome 显示授权提示时让用户手动允许，不绕过授权、不抢前台、不重启 Chrome，也不复制 profile。已有调试连接时，先识别持有连接的服务，不额外创建一批连接或停止未知进程。
6. 日志、PID 和端口对应清楚后，只修正已证实的问题。原命令超时时先核对是否仍在执行及已有输出，不能自动重发。确需停止已确认属于本任务的空闲 daemon 或切换 profile 时，先说明影响并取得用户确认；完成后只重试一次只读查询。

向用户说明已查到的状态、失败阶段和缺少的证据。未看到原请求成功结果前，不要声称连接已恢复或数据已采集。
