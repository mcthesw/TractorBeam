# 路线图

[English](roadmap.en.md)

Windows + Steam 基线和面向玩家的测试目标已经完成。后续工作将优先保持现有体验稳定，同时欢迎社区参与范围清晰的改进和未来方向。

## 已完成的基线

- [x] 提供 Windows Client Bundle，包括 Bridge GUI、Bridge Client、Native Hook 和 Injector。
- [x] 支持 Steam + *以撒的结合：忏悔+*，并实现 Official Mode、Fallback Mode 和 Pure Mode。
- [x] 支持外部 Relay 和局域网直连，以及可选择的 TCP/UDP Relay Transport。
- [x] 提供可自部署的 Relay Server、基础滥用限制、日志、指标、Trace 和运维文档。
- [x] 提供 Diagnostics Bundle、日志脱敏、玩家错误提示和常见故障恢复路径。
- [x] 提供 GitHub Release 资产、Release Please 流程和可在干净 Windows 设备运行的 Client Bundle。

## 当前重点

- 修复可复现的严重 Bug、安全问题和兼容性回归。
- 保持 Windows + Steam 基线、Relay 自部署和现有协议路径可维护。
- 改进玩家文档、诊断证据和范围清晰的小型体验问题。
- 在时间允许时审阅边界明确、验证充分的社区贡献。

贡献前请阅读[贡献指南](CONTRIBUTING.md)。

## 未来方向

- Directory Service、签名 Relay 元数据、撤销和信任发布。
- Client/Relay 协议最低与最高版本策略。
- 长期公共 Relay Server 策略。
- 有界 UDP 重复发送/去重、逐跳 FEC 及其带宽、尾延迟和 Relay CPU 测量。
- Linux/Proton、非 Steam 支持和安装程序打包。
- 动态 Input Delay 和更多联机质量可视化。

这些方向目前不是优先事项，欢迎有兴趣的贡献者先通过 Issue 讨论范围和验证方式。涉及协议、Relay 数据路径、Native Hook 或 Injector 的大型工作，也应先通过 Issue 对齐范围再开始实现。

载荷加密仍不在当前范围内。
