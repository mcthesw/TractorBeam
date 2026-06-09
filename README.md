# Basement Bridge

[English](README.en.md)

给 *以撒的结合：忏悔+* 远程联机用的桌面客户端和中继传输。

官方联机很卡，走虚拟局域网也经常不流畅，因此这里我们引入非p2p方案，在保留steam版所有正常功能前提下，把游戏数据传输改为走服务器中继，实现较好的传输质量。

目前仍在开发中，只支持 Windows + Steam。

## 构建

```sh
cargo check --workspace
cargo test --workspace
```

## 文档

- [docs/architecture.md](docs/architecture.md)：组件边界和数据流。
- [docs/relay.md](docs/relay.md)：Relay Server 部署。
- [docs/security.md](docs/security.md)：威胁模型和安全边界。
- [roadmap.md](roadmap.md)：阶段规划。

## 许可证

默认采用 [GNU AGPL v3.0 or later](LICENSE)。如需其他授权、商业使用或例外许可，
请通过作者 GitHub 主页的公开联系方式联系。
