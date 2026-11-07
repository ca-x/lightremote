# 独立 daemon 与系统服务

`lightremote-daemon` 是独立可执行程序，默认构建不包含桌面捕获、输入和剪贴板依赖。
它复用 `lightremote::host` 的会话、权限、worker 清理、私有邀请发布、网络恢复和空闲更新逻辑。
每次会话结束后重新创建身份和单次邀请，不自动重放远程操作。

```sh
cargo build --release -p lightremote-daemon
lightremote-daemon --config /absolute/private/daemon.json --check-config
lightremote-daemon --config /absolute/private/daemon.json
```

此处只提供模板和显式安装步骤；构建、验证配置均不注册或启动系统服务。
现有 `deploy/host/` 是运行 CLI host 的兼容方式。Windows 用户登录任务只能在登录后运行，
不等同于 SCM 服务。Windows 真正服务使用本 daemon 的 `--service` 入口。

## 配置和权限

从 `deploy/daemon/daemon.example.json` 复制配置。JSON 顶层必须是对象，最大 64 KiB；
未知字段、重复字段、错误类型、注释、尾随内容均拒绝。所有路径必须为绝对路径且不能含 `..`。
`--config PATH` 必须显式提供，不从工作目录搜索配置。配置和 state 路径拒绝符号链接，
Windows 也拒绝 reparse points。配置文件必须是普通私有文件，Unix 拒绝额外硬链接。

Unix 配置建议 `0600`、配置目录和 state 目录 `0700`，所有者必须为服务账户或 root。
祖先目录不能由其他用户写入；允许 `/tmp` 这类 sticky 系统目录作为祖先，但 state 本身必须私有。
Windows 只允许服务账户、SYSTEM 和 Administrators 访问配置及 state；父目录拒绝其他主体写入。
ACL 检查通过系统 PowerShell 只读执行，无法读取 ACL 时拒绝启动。模板默认服务账户为 SYSTEM；
安装脚本预检固定采用 SYSTEM/Administrators 契约，不把运行安装器的个人 SID 视为服务账户。
改用专用账户需要单独配置服务及其私有 ACL，并以该账户进行最终校验；此安装脚本只注册 LocalSystem。

令牌只从 `LIGHTREMOTE_RELAY_TOKEN` 环境变量或私有 JSON 的 `relay_token` 字段读取。
环境变量存在时覆盖配置；空值也会覆盖并导致校验失败。命令行不接受令牌。
令牌必须为非空可打印 ASCII，最多 4096 字节。不要把凭证写进公开 unit/plist、命令历史或日志。
普通服务安装建议直接使用私有 JSON，避免服务管理器显示环境值。

daemon 始终采用 `unattended=true` 和 `stay=true`，必须显式设置 `policy_confirmed=true`。
这意味着你确认所有勾选能力将成为长期无人值守授权的上限；示例配置故意保持未确认。
`allow_*` 全部默认 `false`，`share_dir` 默认 `null`；指定 share 才开放目录读取，
`allow_write` 还需单独开启。`allow_shell` 允许远端以服务账户执行命令，尤其要谨慎授予 SYSTEM/root。
`allow_input` 要求 `allow_desktop=true`。默认文件大小上限 1 GiB、16 个并发流、I/O 超时 60 秒；
流数范围为 1–64，超时范围为 1–300 秒。`shell` 可指定本地绝对可执行文件路径。
远端 relay 和 Web 客户端地址要求 TLS，只有本机回环地址允许明文开发连接。

`network` 为 `default` 或 `slow`；后者将连接预算设为 180 秒、打开/写入 60 秒、
心跳间隔 20 秒/超时 90 秒、降级阈值 10 秒、排空 120 秒。预算只影响本地等待，
不会改变传输协议，也不保证桌面实时性。

`--check-config` 检查 JSON、授权确认、令牌、路径、现有目录权限及能力约束，
不连接 relay、不检查更新、不创建 state/邀请文件、不构建 host services、不连接或注册 SCM。
state 可以尚不存在，但它的父目录必须已存在并安全。检查成功不代表远端可达、
账户能打开 PTY、共享目录可写或桌面系统可用。

正常启动会创建单层 `state_dir`（Unix `0700`），在其下自动使用固定的 `invite` 私有文件。
daemon 把共享 host 的 `Options.session_gate_path` 设置为 `state_dir/sessions.lock`，
该永久锁覆盖会话准入和更新排他；无需 HOME、XDG_STATE_HOME 或登录用户的状态目录。
共享 host 的这个选项默认是 `None`，CLI 等调用方继续使用它们自己的用户状态目录契约。
邀请通过共享引擎原子发布（Unix `0600`），等待结束或正常停止时撤销。
服务日志不输出邀请链接、令牌或网络错误详情。仅通过本地受控方式读取邀请文件，勿采集其内容。
异常断电/强杀可能留下旧文件；下一次启动会验证权限并以新身份覆盖，旧文件不是持久配对。

## Linux systemd

安装前创建专用账户和私有配置（以下操作由管理员显式执行）：

```sh
sudo useradd --system --home-dir /var/lib/lightremote-daemon --shell /usr/sbin/nologin lightremote
sudo install -d -m 0700 -o lightremote -g lightremote /etc/lightremote-daemon
sudo install -m 0600 -o lightremote -g lightremote deploy/daemon/daemon.example.json /etc/lightremote-daemon/daemon.json
sudo install -m 0755 target/release/lightremote-daemon /usr/local/bin/lightremote-daemon
# 使用编辑器设置真实 URL、relay_token、policy_confirmed 和明确的权限。
sudoedit /etc/lightremote-daemon/daemon.json
sudo -u lightremote /usr/local/bin/lightremote-daemon --config /etc/lightremote-daemon/daemon.json --check-config
sudo install -m 0644 deploy/daemon/lightremote-daemon.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now lightremote-daemon
sudo systemctl status lightremote-daemon
# 正常停止触发 SIGTERM，并等待共享 host engine 清理任务。
sudo systemctl stop lightremote-daemon
```

unit 的 `StateDirectory` 创建 `/var/lib/lightremote-daemon`，并通过 `UMask=0077` 保持私有。
`ProtectSystem=strict`、`ProtectHome=true` 和非特权账户是模板默认约束；需要共享目录时
选择账户可访问的位置，写入还需在 unit 增加精确的 `ReadWritePaths`，不要直接解除全部保护。
模板 `Restart=on-failure` 和 `RestartForceExitStatus=75` 处理崩溃、故障及更新后的重启；
正常 SIGTERM 退出 0，不触发故障重启。

## macOS launchd

模板 `deploy/daemon/org.lightremote.daemon.plist` 是系统 LaunchDaemon，直接执行 daemon。
管理员先通过系统账户管理方式创建专用 `_lightremote` 账户，再安装二进制，创建以下目录：

```sh
sudo install -d -m 0700 -o _lightremote '/Library/Application Support/LightRemote'
sudo install -m 0600 -o _lightremote deploy/daemon/daemon.example.json '/Library/Application Support/LightRemote/daemon.json'
sudo touch /var/log/lightremote-daemon.log
sudo chown _lightremote /var/log/lightremote-daemon.log
sudo chmod 0600 /var/log/lightremote-daemon.log
```

编辑配置，将 `state_dir` 设为 `/Library/Application Support/LightRemote/state`，
设置令牌、URL 和授权确认。该目录父级由服务账户持有，daemon 会创建私有 state。
以 `_lightremote` 执行 `--check-config` 后，管理员显式安装和启动：

```sh
sudo -u _lightremote /usr/local/bin/lightremote-daemon --config '/Library/Application Support/LightRemote/daemon.json' --check-config
sudo install -m 0644 -o root -g wheel deploy/daemon/org.lightremote.daemon.plist /Library/LaunchDaemons/
sudo launchctl bootstrap system /Library/LaunchDaemons/org.lightremote.daemon.plist
sudo launchctl print system/org.lightremote.daemon
sudo launchctl bootout system/org.lightremote.daemon
```

`KeepAlive.SuccessfulExit=false` 重启非零退出，包括更新退出 75；`bootout` 用于明确停止。
系统 LaunchDaemon 不等于已获得登录用户的屏幕录制/辅助功能授权，桌面请使用用户会话 CLI host。

## Windows 原生 SCM 服务

`--service` 使用 `windows-service` 连接 SCM，服务名固定为 `LightRemoteDaemon`，
报告 StartPending、Running、StopPending、Stopped，接受 STOP 和 SHUTDOWN。
停止控制完成 shutdown future，由共享引擎撤销权限、停止并等待 worker，而后报告 Stopped。
Running 表示本地配置加载完成、运行时已启动，不表示 relay 已联网。
配置校验失败会报告非零服务退出；用 `sc.exe queryex LightRemoteDaemon` 查看状态和退出码。
普通终端不加 `--service`，可用 Ctrl+C 停止；Unix 控制台模式同时支持 SIGTERM。

以下命令需要用户主动打开**管理员 PowerShell**；脚本不会静默提权。
先将 daemon 放在管理员保护的 `C:\Program Files\LightRemote`，以防服务二进制被替换。
私有配置位于 `C:\ProgramData\LightRemote`。示例 SYSTEM 服务拥有较高权限，
默认不开放 shell、共享目录或桌面；专用账户需要独立安装流程。
安装脚本在首次执行 daemon 前逐级检查 binary/config/state 路径的 owner、DACL 和 reparse 属性。
二进制及其父目录不能被普通用户修改或替换，配置与 state 只允许 SYSTEM/Administrators；
既有 Windows 系统祖先可由 TrustedInstaller 持有，但不能允许普通用户删除/替换受保护子项。
仅支持无 `..`、尾随点/空格、ADS 的本地固定磁盘绝对路径，不接受 UNC、设备路径或 junction/symlink。

```powershell
New-Item -ItemType Directory -Force 'C:\ProgramData\LightRemote'
icacls.exe 'C:\ProgramData\LightRemote' /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
Copy-Item deploy\daemon\daemon.example.json 'C:\ProgramData\LightRemote\daemon.json'
icacls.exe 'C:\ProgramData\LightRemote' /setowner '*S-1-5-32-544' /T
# 编辑 JSON：state_dir = "C:\\ProgramData\\LightRemote\\state"，设置 URL、relay_token、policy_confirmed。
& 'C:\Program Files\LightRemote\lightremote-daemon.exe' --service --config 'C:\ProgramData\LightRemote\daemon.json' --check-config
.\deploy\daemon\windows-service.ps1 -BinaryPath 'C:\Program Files\LightRemote\lightremote-daemon.exe' -ConfigPath 'C:\ProgramData\LightRemote\daemon.json'
sc.exe start LightRemoteDaemon
sc.exe queryex LightRemoteDaemon
sc.exe stop LightRemoteDaemon
# 停止后才删除注册：
sc.exe delete LightRemoteDaemon
```

上述 ACL 示例针对新建专用目录；若目录已经有显式的宽泛 ACE，`/grant:r` 不会删除其他主体，
应先检查 `icacls.exe` 输出并移除不受信任的 ACE，校验失败时不要放宽 daemon 检查。
配置文件从目录继承私有 ACL；state 必须同样私有。Windows ACL 检查需要系统自带 Windows PowerShell。
安装者的个人 SID 即使属于管理员，也不能作为配置/state 的 owner 或额外获准主体；
请使用示例中的 Administrators **组** SID 或 SYSTEM。state 不存在时，预检检查其父目录的私有及继承 ACL，
不会创建 state；已有 state 的邀请和永久锁也须满足同一契约。预检不会修复 ACL 或静默提权。

安装脚本的核心操作是明确的 `sc.exe create ... binPath= '"...exe" --service --config "...json"' start= delayed-auto obj= LocalSystem`，
随后设置 `sc.exe failure ... actions= restart/5000/restart/15000/restart/60000`，
以及 `sc.exe failureflag LightRemoteDaemon 1`。后者让服务报告非零 Stopped（包括 75）也应用恢复策略。
脚本仅注册，不启动；若后续恢复策略配置失败，已有服务注册不会自动回滚，管理员应检查并重配或删除。

SCM 服务运行在 session 0。即使显式编译 `--features desktop`，SCM 模式也拒绝所有桌面、
输入和剪贴板权限，不声称可控制登录用户桌面。桌面继续运行登录用户会话内的 `lightremote host`。
本实现不执行提权、跨 session 注入、TCC 授权或系统凭证管理。

## 更新与验证边界

`auto_update` 默认关闭。开启后使用 daemon 专属 `Product::Daemon` 官方 GitHub 架构产物，
共享更新门锁只允许无会话时安装，成功后退出 75，由服务管理器重启。
官方 daemon 产物为 headless；即使自行使用 `--features desktop` 编译，配置也拒绝
`auto_update=true` 与任意桌面、输入或剪贴板权限同时启用，避免更新后因缺少桌面支持而无法启动。
这类构建应关闭自动更新，由管理员维护匹配的二进制。
进程不会自行脱离 supervisor 启动第二份 daemon。更新失败保持现有二进制并按共享引擎周期再检查。
服务账户还必须拥有替换二进制及其父目录的权限；默认 systemd 模板和管理员安装位置故意不提供此权限，
因此默认配置适合管理员更新。启用自更新前需要单独设计可写安装目录及沙箱例外，并保护该目录免受其他用户修改。
管理员手动替换二进制前，必须先停止使用该安装的全部 daemon/host 实例并等待退出，
不能假定使用其他默认 gate 路径的更新命令会与 daemon 的 `state_dir/sessions.lock` 自动互斥。
保留 `sessions.lock` 和邀请 writer lock 等永久锁文件，不得删除、改名或替换来绕过忙碌状态；
同一把锁的文件身份必须跨重启保持稳定。

crate 测试覆盖默认拒绝策略、严格/有界配置、Unix 文件权限与链接、检查命令无网络和 state 副作用、
真实子进程经历本地 relay 不可达后恢复，以及 SIGTERM 撤销私有邀请和无凭证日志。
协议 fixture 只模拟房间回复并保持 WebSocket 握手等待；不把它称作真实桌面或 Windows SCM 测试。
可手动运行真实本地中继联调：

```sh
cargo build --locked -p lightremote-daemon -p lightremote-relay -p lightremote --no-default-features
python3 crates/daemon/tests/e2e.py
```

它连接真实 relay，完成两次 capabilities 会话，验证每次新邀请、默认关闭权限与正常停止。
Windows 交叉编译只能验证类型和条件编译，SCM 状态、ACL 与恢复策略仍需 Windows 主机实测。

Windows 安装预检另有无需注册服务的原生 PowerShell 测试，可在 hosted Windows runner 上直接运行：

```powershell
powershell.exe -NoLogo -NoProfile -NonInteractive -File deploy\daemon\tests\windows-service.ps1
```

测试通过 Parser 只载入安装脚本的函数定义，不执行 daemon 或安装语句。它验证固定服务 SID 的
ACL 描述符、NULL DACL、写入/替换权限、个人 owner 和真实临时可写路径拒绝；已有管理员 token 时
在新建的临时子目录内再验证受保护的安全路径、可写 file/parent、私有 state、个人 SID owner 和 junction。
未持有管理员 token 时不会请求提权，会明确输出受保护目录测试的 SKIP，只运行描述符及真实不可信路径 canary。
测试仅修改并清理自己的临时文件/ACL，不改变系统目录 ACL，也不调用 SCM；不能将其视为 SCM 生命周期实测。
