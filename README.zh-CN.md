# LightRemote

基于 TMP 的远程桌面、终端和文件传输项目，提供 Web 与 NSH 原生客户端。
主机程序与自建中继使用 Rust，也支持 Cloudflare Workers + Durable Objects 中继。

当前是 0.1.1 初版。桌面视频使用实验性 JPEG 流；编译、自动化联调、真实桌面
验证和签名状态分别记录在 [CAPABILITIES.md](CAPABILITIES.md)。桌面、Shell、
文件、剪贴板和截图都是应用层 profile，不修改 TMP 通用协议。

本分支正在完善下一版：逐项主机审批与撤权、弱网健康状态和低带宽预设、GitHub
更新，以及独立的 `lightremote-daemon` 服务。这些改动尚未作为新 Release 发布；
当前 GitHub 正式下载仍为 v0.1.1。

![Web 工作台](docs/screenshots/web-desktop.png)

从 [GitHub Releases](https://github.com/lib-x/lightremote/releases) 下载编译后的包：
`lightremote-native` 是 NSH 原生控制端，`lightremote` 是桌面主机与 CLI，
`lightremote-headless` 是不依赖 GUI 的 Shell/文件主机，`lightremote-relay` 是中继，
`lightremote-web` 是 Web 静态资源；二进制包名包含系统和架构。
桌面 CLI 和 headless CLI 内的程序都叫 `lightremote`，请解压到不同目录。

本地启动：

```sh
cd web && npm ci && npm run build && cd ..
cargo build -p lightremote-relay -p lightremote --no-default-features
export LIGHTREMOTE_RELAY_TOKEN="$(openssl rand -hex 32)"
LIGHTREMOTE_WEB_DIR=web/dist ./target/debug/lightremote-relay
```

另一个终端设置相同的 `LIGHTREMOTE_RELAY_TOKEN`，启动主机：

```sh
./target/debug/lightremote host \
  --relay http://127.0.0.1:8787 --web-url http://127.0.0.1:8787/ \
  --share-dir "$HOME/Downloads" --allow-shell --allow-write
```

浏览器打开打印的 `INVITE` 链接。邀请仅供一个控制端、一次会话使用；断线后由主机
重新创建邀请。主机权限默认关闭，文件访问限制在共享目录中，上传拒绝覆盖已有文件。
启用 Shell 就授予了运行主机进程的系统用户权限。

新代码默认要求本地终端审批：控制端在权限面板中申请能力，主机输入
`approve ID [scope ...]` 或 `deny ID`；可用 `revoke all` 撤销。
非交互主机必须显式加 `--unattended`，仅授予配置旗标允许的能力。
`--network slow` 使用较长但有上限的连接、写入和 RPC 预算；桌面低带宽预设为
3 fps / JPEG quality 40，只影响下一次打开画面。

后台服务使用独立程序，见 [daemon 配置与部署](docs/daemon.md)。默认 daemon
为 headless，不依赖桌面环境；Windows SCM 服务不直接捕获登录用户的桌面。
交互主机可用 `host --stay` 保持运行，每轮生成新邀请；后台邀请通过私有文件交付。

`lightremote update` 检查官方 GitHub 正式 Release，`lightremote update --install`
校验并安装；原生客户端提供自动检查开关和空闲时安装。常驻主机可显式启用
`--auto-update`，仅在会话之间安装，退出码 75 交给服务管理器重启。
详见 [更新与恢复](docs/updating.md) 和 [交互 CLI 常驻说明](docs/resident-host.md)。

桌面主机执行 `cargo build -p lightremote`，启动时增加 `--allow-desktop`、
`--allow-input`，需要剪贴板时增加 `--allow-clipboard`。原生客户端执行
`cargo run -p lightremote-native`。Linux 的 GTK4、GStreamer、Clang/PipeWire
依赖，以及 macOS/Windows 的权限边界见 [平台说明](docs/platforms.md)。

[英文 README](README.md) 提供 CLI 控制端用法。
[Docker 部署](deploy/README.md)、[Cloudflare 部署](cloudflare/README.md)、
[分发与发布](docs/distribution.md)、[应用协议](docs/design.md)、
[独立审查记录](docs/review.md) 和 [实现审查](docs/implementation-review.md)
包含更详细的合同与证据。
下一版权限、常驻和更新的修复与验证见 [本轮评审记录](docs/hardening-review.md)。

邀请中的随机密钥用于认证邀请持有者并加密 TMP。首版没有持久设备身份和前向保密；
使用不可信中继时，应从另一个可信 HTTPS 来源提供 Web 客户端。所有服务目前共用
有序承载，大文件可能影响输入与画面延迟。Cloudflare 中继有明确的会话流量配额。
尚未完成的桌面真机测试、签名、公证或实际 CF 账户部署不会被记为已完成。

许可证为 MIT；参考来源与依赖许可见 [THIRD_PARTY.md](THIRD_PARTY.md)。
