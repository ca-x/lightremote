# 常驻主机：显式权限与用户级服务

安装为服务时优先使用独立的 [`lightremote-daemon`](daemon.md)。本页保留 CLI
`host --stay` 的用户会话和兼容部署方式，Windows 定时任务不是 SCM 服务。

`lightremote host --stay` 在每个会话结束后清理服务任务和输入状态，并生成新的 PSK、单次 ticket 和邀请。它不恢复旧会话，不重复消费旧 ticket，不重放键盘、终端命令、文件提交或剪贴板写入。弱网重试使用有界退避；对端失联后必须取得新邀请。常驻不保证机器休眠、注销或断网时仍可访问。

CLI 默认是 attended，需要本地 TTY 审批；`--stay` 本身不取消审批。后台服务没有交互终端，必须显式使用 **`--unattended`**。此时配置的每个权限旗标都是跨会话、跨进程重启的长期授权政策，邀请持有人可直接使用已启用的能力。先选择最小权限，再确认政策，最后安装服务。模板中的 `LIGHTREMOTE_POLICY_CONFIRMED=no` / `PolicyConfirmed=false` 会阻止启动；全部能力默认关闭，自动更新也默认关闭。

这里只提供源码模板，安装命令须由用户自行运行；不会自动写入系统目录、注册任务、提升权限或安装服务。以下 Unix 命令从仓库根目录运行；`/PATH/TO/...`、`/ABSOLUTE/USER/PATH`、示例域名与令牌均须替换。不要使用系统管理员身份运行远程主机。

## 选择长期权限

| CLI 选项 | Unix 环境变量 / Windows JSON 字段 | 授权含义 |
| --- | --- | --- |
| `--share-dir PATH` | `LIGHTREMOTE_SHARE_DIR` / `ShareDir` | 非空即授权读取选定目录；留空关闭文件访问 |
| `--allow-write` | `LIGHTREMOTE_ALLOW_WRITE` / `AllowWrite` | 向共享目录写入；必须同时设置 share-dir |
| `--allow-shell` | `LIGHTREMOTE_ALLOW_SHELL` / `AllowShell` | 以当前 OS 用户执行 shell，权限很大 |
| `--allow-desktop` | `LIGHTREMOTE_ALLOW_DESKTOP` / `AllowDesktop` | 读取桌面画面 |
| `--allow-input` | `LIGHTREMOTE_ALLOW_INPUT` / `AllowInput` | 控制主机键盘和鼠标 |
| `--allow-clipboard-read` | `LIGHTREMOTE_ALLOW_CLIPBOARD_READ` / `AllowClipboardRead` | 远端读取主机剪贴板 |
| `--allow-clipboard-write` | `LIGHTREMOTE_ALLOW_CLIPBOARD_WRITE` / `AllowClipboardWrite` | 远端改写主机剪贴板 |
| `--allow-clipboard` | `LIGHTREMOTE_ALLOW_CLIPBOARD` / `AllowClipboard` | 同时允许两个剪贴板方向 |
| `--auto-update` | `LIGHTREMOTE_AUTO_UPDATE` / `AutoUpdate` | 空闲时安装 GitHub 正式 Release；要求 `--stay` |

Unix 权限变量只接受 `0` 或 `1`，确认变量只接受 `yes`；Windows JSON 使用布尔值 `false` / `true`，不能写字符串 `"false"`。只读文件服务仅设置 share-dir；只开画面不代表授予输入。shell 不受 share-dir 文件能力边界限制，仍具有此 OS 用户的一般访问能力。桌面、输入和剪贴板还需要 desktop 构建、图形会话及系统隐私授权；模板不会代替授权，也不会为无头构建模拟支持。

本地交互审批可在终端使用 `lightremote host --stay --invite-file /PRIVATE/PATH/invite`，另外提供 relay、web-url 和选定权限；令牌通过 `LIGHTREMOTE_RELAY_TOKEN` 注入。不要将 attended 命令放入这里的后台服务模板。

## 邀请、令牌和日志

`--invite-file PATH` 原子发布最新邀请，邀请包含会话 secret，必须放在当前用户私有目录。Unix 状态目录权限 `0700`、文件 `0600`、umask `077`；Windows 状态目录及文件 ACL 仅授予当前用户，管理员仍可能接管。不要放进共享目录、公共临时目录、云同步目录或 Web 根目录，不要创建符号链接或 junction。保持目录的父路径可信。

只有当前的单次邀请可用；文件内容可能在会话切换时被替换。消费者应重新打开文件读取，不要长期缓存打开的文件描述符。通过经过认证的私有渠道交付邀请；不要 `cat` 到终端录屏、`echo` 到 stdout、上传到 issue，或用 journal 查看邀请。服务状态日志不传递邀请。停止后如仍有旧文件，按卸载步骤删除；旧邀请不能恢复已经结束的会话。

`LIGHTREMOTE_RELAY_TOKEN` 是 relay operator credential，只写入用户私有配置；不放入命令行或任务定义。Unix 环境文件不提供加密存储，同 UID 进程仍可访问环境。macOS 模板把令牌放在私有 plist 的环境段，`launchctl print` 可能显示配置，不能把其输出交给公开日志。Windows JSON 同样是私有明文配置，不能使用 PowerShell transcription/debug 输出读取它。

Unix 启动器只向 manager 输出固定的启动/配置状态，CLI stdout/stderr 全部丢弃；Windows 并发排空两个输出流到 `Stream.Null`，不保存内容。这样不会因错误消息包含远端数据而泄漏到 journal。可用退出码、进程状态和邀请文件的存在/修改时间检查状态；不要启用 `set -x` 或打印环境。Unix 禁止 core dump，并移除继承的调试日志变量。Windows 崩溃报告/内存转储属于 OS 配置，模板不修改它们；转储可能包含 secret。

## Linux：systemd user service

使用用户可写的独立安装副本，避免自动更新去替换 root 所有的文件或包管理器管理的文件。先准备文件，尚不启动服务：

```bash
umask 077
install -d -m 700 "$HOME/.local/bin" "$HOME/.local/lib/lightremote-host" \
  "$HOME/.config/lightremote-host" "$HOME/.local/state/lightremote-host" \
  "$HOME/.config/systemd/user"
install -m 700 /PATH/TO/VERIFIED/lightremote "$HOME/.local/bin/lightremote"
install -m 700 deploy/host/run-host.sh "$HOME/.local/lib/lightremote-host/run-host.sh"
install -m 600 deploy/host/EnvironmentFile.example "$HOME/.config/lightremote-host/host.env"
install -m 600 deploy/host/lightremote-host.service "$HOME/.config/systemd/user/lightremote-host.service"
"${EDITOR:-vi}" "$HOME/.config/lightremote-host/host.env"
```

在编辑器中填入真实 HTTPS relay/web URL、operator token，逐项选择权限；最后将 `LIGHTREMOTE_POLICY_CONFIRMED` 改为 `yes`。`EnvironmentFile` 是 systemd 语法，不是 shell：值中的 `$HOME`、`~` 不会展开；自定义路径必须为实际绝对路径，包含空格时使用双引号。不要把环境文件 `source` 到 shell。默认程序路径是 `$HOME/.local/bin/lightremote`，默认状态目录是 `$HOME/.local/state/lightremote-host`，由启动器展开。

```bash
systemd-analyze --user verify "$HOME/.config/systemd/user/lightremote-host.service"
systemctl --user daemon-reload
systemctl --user enable --now lightremote-host.service
systemctl --user is-active lightremote-host.service
systemctl --user show lightremote-host.service -p MainPID -p ExecMainStatus -p NRestarts
journalctl --user -u lightremote-host.service --since today
```

`Restart=always` 覆盖正常退出、失败和更新退出码 `75`；重启间隔 10 秒，5 分钟最多 10 次启动，配置错误 `64` 不自动重试。修正配置后运行 `systemctl --user reset-failed lightremote-host.service` 和 `systemctl --user restart lightremote-host.service`。不要把 `75` 设置成成功且禁止重启的状态。

停止命令会先传 SIGTERM：启动器使用 `exec`，CLI 直接接收信号，取消会话、连接尝试及退避并清理；30 秒未结束则 systemd 杀死整个 control group。使用 manager 停止，不要只 kill 主 PID，否则自动重启仍生效。

```bash
systemctl --user stop lightremote-host.service
# 修改长期权限后重新启动，现有会话会结束：
systemctl --user restart lightremote-host.service
# 卸载服务：
systemctl --user disable --now lightremote-host.service
rm "$HOME/.config/systemd/user/lightremote-host.service"
systemctl --user daemon-reload
# 确认不再需要配置和邀请后删除（程序副本单独保留）：
rm -rf "$HOME/.config/lightremote-host" "$HOME/.local/state/lightremote-host" \
  "$HOME/.local/lib/lightremote-host"
```

默认跟随用户登录生命周期。需要注销后持续运行时，用户可另行显式执行 `loginctl enable-linger "$USER"`；它可能需要本机政策授权，不由模板执行。撤销使用 `loginctl disable-linger "$USER"`，但先确认没有其他 user service 依赖 linger。linger 不会创建可用桌面会话，Wayland/PipeWire 等环境需独立验证；无头服务应关闭 desktop/input/clipboard。

## macOS：用户 LaunchAgent

使用系统 `/bin/bash` 可运行的启动器（兼容 Bash 3.2）。不写 `/Library/LaunchDaemons`，不使用 sudo。模板中 launchd 不展开 `~` 或 `$HOME`，将所有 `/ABSOLUTE/USER/PATH` 替换为实际用户目录；XML 值中的 `&` 需写成 `&amp;`。

```bash
umask 077
install -d -m 700 "$HOME/.local/bin" "$HOME/.local/lib/lightremote-host" \
  "$HOME/Library/Application Support/LightRemoteHost"
mkdir -p "$HOME/Library/LaunchAgents"
install -m 700 /PATH/TO/VERIFIED/lightremote "$HOME/.local/bin/lightremote"
install -m 700 deploy/host/run-host.sh "$HOME/.local/lib/lightremote-host/run-host.sh"
install -m 600 deploy/host/org.lightremote.host.plist "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
"${EDITOR:-vi}" "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
plutil -lint "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
# 确认路径、URL、令牌、权限并把政策确认值改为 yes 后：
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
launchctl kickstart "gui/$(id -u)/org.lightremote.host"
```

`KeepAlive=true` 在包括 `75` 的退出后重启，节流间隔 10 秒；配置错误也会重试，须先 bootout 再修复。LaunchAgent 仅在用户 GUI 登录期间运行，不承诺注销后常驻。画面读取、输入和剪贴板需实际 GUI 及 macOS 权限验证，不应把 launchd 注册成功当成这些权限可用。

修改政策、停止或卸载都先 bootout；它发送 SIGTERM，允许最多 30 秒清理。不要将包含环境的 `launchctl print` 输出写入日志。

```bash
launchctl bootout "gui/$(id -u)" "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
# 修改 plist 后重启：编辑、plutil -lint，再 bootstrap。
# 完整卸载（bootout 后）：
rm "$HOME/Library/LaunchAgents/org.lightremote.host.plist"
rm -rf "$HOME/Library/Application Support/LightRemoteHost" "$HOME/.local/lib/lightremote-host"
```

## Windows：普通用户定时任务

模板面向 Windows PowerShell 5.1 和 ScheduledTasks 模块。使用普通用户 PowerShell，不请求管理员，不保存登录密码，不使用 `RunLevel Highest`，不自动修改 ExecutionPolicy。若系统禁止脚本，由用户按组织策略签名或允许这些已审核脚本；命令不附带 bypass。请使用 Windows PowerShell 的 `powershell.exe` 执行安装器。

从仓库根目录执行 Prepare，它仅创建 `%LOCALAPPDATA%\LightRemoteHost`，去掉继承权限并仅给当前用户 FullControl，复制 runner 和未确认的配置。目录之外的父路径必须可信；不支持重解析目录。此时还没有任务或进程。

```powershell
powershell.exe -NoProfile -File .\deploy\host\install-host.ps1 -Action Prepare
notepad.exe "$env:LOCALAPPDATA\LightRemoteHost\host.json"
```

自行将可信 Release 的 `lightremote.exe` 放进当前用户可写目录，配置 `BinaryPath` 的完整路径（JSON 中反斜杠写成 `\\`），填入 HTTPS URL 和 token，选择各权限，最后设置 `PolicyConfirmed: true`。邀请固定写入私有状态目录的 `invite`。不要把二进制或更新临时文件放到启用的 share-dir 中。

```powershell
# 明确安装并立即启动，之后在当前用户登录时运行：
powershell.exe -NoProfile -File .\deploy\host\install-host.ps1 -Action Install
Get-ScheduledTask -TaskName LightRemoteHost | Select-Object TaskName, State
Get-ScheduledTaskInfo -TaskName LightRemoteHost | Select-Object LastRunTime, LastTaskResult
# 停止并禁用触发器：
powershell.exe -NoProfile -File .\deploy\host\install-host.ps1 -Action Stop
# 恢复：
Enable-ScheduledTask -TaskName LightRemoteHost
Start-ScheduledTask -TaskName LightRemoteHost
# 修改政策前先 Stop，修改后使用上面的恢复命令。
# 卸载任务（保留配置，便于显式决定是否删除）：
powershell.exe -NoProfile -File .\deploy\host\install-host.ps1 -Action Uninstall
# 确认任务及相关进程停止后删除私有状态、配置和邀请：
Remove-Item -LiteralPath "$env:LOCALAPPDATA\LightRemoteHost" -Recurse -Force
```

任务使用 Interactive 登录类型和 Limited 权限，跟随当前用户登录；不会在注销后或开机未登录时运行。监督脚本按 10 秒间隔重启 CLI，`75` 后等待 15 秒以便更新 helper 替换文件；不会自动重放任何请求。配置错误会停止监督脚本，修正后需要手动 Start-ScheduledTask。不要重复注册同名任务，重新安装先 Uninstall。任务状态 Running 指监督脚本存在，不等于已连上 relay。

Windows 的任务终止是强制终止，不能当作 Unix SIGTERM。runner 正常退出时会清理直接子进程；Task Scheduler 强制结束时不保证 PowerShell finally 执行。必须在真实 Windows 上确认任务结束是否完整终止 CLI、PTY/shell 子进程、释放输入、清理邀请/上传临时文件；目前不能承诺该路径已完成优雅清理。停止后检查相关进程，不要因为停止监督脚本就认定远程会话已经结束。必要时用户在任务管理器终止对应进程树。

## 自动更新与验证边界

只以固定 `lib-x/lightremote` GitHub 正式 Release 为更新源，选择稳定版本、正确产品和 OS/架构资产，校验下载内容。模板不会从分支、Actions 临时产物或可变第三方 latest URL 安装。TLS/GitHub 资产摘要是现有分发信任边界，不声称已有发布者签名或 OS 签名。

`--auto-update` 必须与 `--stay` 一起使用，三个模板均已带 `--stay`。更新须等待活动会话结束；成功安装后 CLI 退出 `75`，由 systemd/launchd 或 Windows 监督脚本重新拉起。二进制父目录必须允许当前用户创建/替换文件；失败应保留现有可执行文件。Windows 可能先安排 helper 等 CLI 退出后替换，因此 15 秒仅是监督重启宽限，不是替换成功证明，真实 Windows 仍需验证替换时序、文件锁和恢复行为。

部署前在目标系统验证：确认无权限配置拒绝未授权请求；正常断开后新邀请变化且旧邀请失效；断网再恢复不会重放副作用；停止时会话/输入/上传清理；启用自动更新后只在空闲安装并经 `75` 重启；输出中没有 token、邀请或远端内容。可记录邀请文件修改时间，不能把邀请值写入验证报告。不要为了验证更新去改动不可变 Release tag。

静态检查可用：`bash -n deploy/host/run-host.sh`、`shellcheck deploy/host/run-host.sh`、`systemd-analyze --user verify deploy/host/lightremote-host.service`、macOS `plutil -lint`。Windows 可通过 `[System.Management.Automation.Language.Parser]::ParseFile(...)` 检查 PowerShell 语法，必须另外在实际 Windows 验证 ACL、任务注册、停止及更新 helper。XML/plist 解析成功不等同于 launchd 生命周期已验证；Linux 环境无 user manager 或示例启动器尚未安装时，应记录 verify 的实际限制，不以安装服务来绕过。

2026-11-07 Linux 本地验证：Bash 语法、ShellCheck 0.11.0、Python plistlib/XML 与 JSON 解析、默认权限关闭和源码凭据/本机路径检查通过。`systemd-analyze verify` 系统模式通过；`--user` 模式报 `Failed to initialize manager: No such device or address`，未安装或启动服务。临时 mock CLI 验证了政策拒绝、权限参数、含空格路径、目录 `0700`、符号链接拒绝、stdout/stderr 丢弃、退出码 `75` 传递以及直接 SIGTERM 传递；这不代替真实 CLI 的会话清理测试。

本环境无 PowerShell、macOS plutil/launchd，尚未运行 PowerShell 官方解析器或真实 OS 生命周期/ACL/更新替换验证。尝试运行 `scripts/check.sh` 时，格式检查和首轮无默认特性 Clippy 通过，后续测试等待共享构建目录锁；为避免与并行集成争抢而终止本次检查，整仓检查未完成。
