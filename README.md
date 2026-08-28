# Tractor Beam

[English](README.en.md)

优化 *以撒的结合：忏悔+* 联机体验的桌面 Client 和 Relay Server。

官方联机或虚拟局域网不够流畅时，Tractor Beam 可以将游戏数据改为通过 Relay 传输，同时保留 Steam 版本的正常功能。

## 项目状态

维护者目前可投入的时间有限，但会尽力修复明确的 Bug 和兼容性问题，并在时间允许时审阅范围清晰的 PR。社区贡献依然欢迎，参见[贡献指南](CONTRIBUTING.md)。

客户端支持 Windows 和 Linux（Proton），游戏版本为 *忏悔+*。Relay 支持 Windows、Linux 和 macOS，正式 Release 提供 Windows 和 Linux 可执行文件。

## 使用 Client

1. 从[最新版本](https://github.com/mcthesw/TractorBeam/releases/latest)下载对应平台的 Client Bundle 并完整解压。
2. 保持解压出的文件位于同一目录：Windows 运行 `tractor-beam.exe`；Linux 运行 `tractor-beam`。
3. 选择 Steam 账号和联机方式。房主复制联机码，其他玩家导入联机码。
4. 点击“启动游戏”。遇到问题时，从 Client 导出 Diagnostics Bundle。

Linux 客户端通过 Proton 运行 Windows 版 *忏悔+*。客户端会在游戏安装目录写入临时 `winmm.dll` 代理，并仅为 `isaac-ng.exe` 临时启用 Wine DLL 覆盖；会话结束或客户端下次启动时会恢复原配置并删除代理。

正式 Client Bundle 需要自行配置 Relay；也可使用单独提供的公测包。公共测试 Relay 由项目维护者自费提供，仅供测试。

- [局域网直连说明](docs/lan.md)
- [Relay 自部署说明](docs/relay.md)

## 构建

需要先安装 Rust 工具链。

```sh
# 构建 Client
cargo build -p tractor-beam-gui

# 构建 Relay Server
cargo build -p tractor-beam-relay

# 检查和测试
cargo check --workspace
cargo test --workspace
```

## 文档

- [docs/architecture.md](docs/architecture.md)：组件边界和数据流。
- [docs/relay.md](docs/relay.md)：Relay Server 部署。
- [docs/relay-configuration.md](docs/relay-configuration.md)：Relay Server 配置。
- [docs/relay-observability.md](docs/relay-observability.md)：Relay Server 日志、指标、Trace 和容量判断。
- [docs/lan.md](docs/lan.md)：局域网与虚拟局域网直连。
- [docs/security.md](docs/security.md)：安全边界。
- [roadmap.md](roadmap.md)：阶段规划。
- [CONTRIBUTING.md](CONTRIBUTING.md)：贡献指南。

## 许可证

默认采用 [GNU AGPL v3.0 or later](LICENSE)。如需其他授权、商业使用或例外许可，请通过作者 GitHub 主页的公开联系方式联系。
