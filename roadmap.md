# 路线图

[English](roadmap.en.md)

路线图将首个里程碑保持在较小范围：先交付已确认可行的 Windows 玩家工具，再扩展平台、打包、传输和安全范围。

## 阶段 1：Windows Rust 基线

目标：保持 Windows Bridge 路径使用 Rust 基线和 Rust Native Hook。

- [x] 仅支持 Windows + Steam + *The Binding of Isaac: Repentance+*。
- [x] 使用运行时 Crate `bridge-core`、`bridge-gui`、`bridge-relay`、 `native-hook` 和 `isaac-injector`，以及职责收窄的共享契约 Crate `hook-ipc` 和 `relay-protocol`。
- [x] 为 i686 Isaac 进程构建 Rust Native Hook DLL。
- [x] 构建 Rust Injector 辅助程序。
- [x] 构建支持房间加入、Peer 转发、UDP/TCP 监听、超时、速率限制和 IP/CIDR 阻止列表的 Rust Relay Server。
- [x] 构建 Rust Bridge Client Runtime，包括异步本地 Hook Bridge、可选 Relay Transport、房间设置、Steam 启动、注入编排、状态和错误处理。
- [x] 构建 egui Bridge GUI，包括 Relay 地址、传输选择、房间、SteamID64、模式、启停、状态、计数器和诊断导出。
- [x] 实现 Official Mode、Fallback Mode 和 Pure Mode。
- [x] 定义首版 Relay 协议 Envelope、版本、能力和错误码。
- [x] 阶段 1 控制消息使用简单的版本化 Envelope。
- [x] 生成基础 Diagnostics Bundle。
- [x] 记录启动、注入、Relay 和 Hook 错误的恢复方式。
- [x] 在协议、Relay 状态和诊断单元测试之外增加有针对性的本地 Bridge 流程测试。
- [x] 增加 Relay Server 运行时计数器和指标。

阶段 1 暂缓：

- Linux 支持。
- 非 Steam 支持。
- 安装程序打包。
- Directory Service。
- 可选的有界 UDP 重复发送/去重或 FEC Profile。

## 阶段 2：测试

目标：让 Windows 基线在真实玩家设备上可靠运行。

- [x] 准备测试说明和反馈模板。
- [x] 部署公共测试 Relay Server。
- [x] 记录 Relay Server 自部署方式。
- [x] 改进 Windows Steam 和 Isaac 路径检测。
- [x] 改进启动、注入、故障恢复和面向用户的错误信息。
- [x] 增加 Relay Server 日志、基础滥用限制和运维手册。
- [x] 定义诊断审阅流程和日志脱敏规则。
- [x] 收集常见 Mod 会话的兼容性说明。
- [x] 增加用于测试运维的 Relay Server 本地 IP/CIDR 阻止列表。
- [x] 在测试者设备上验证 Rust Native Hook 和 i686 Injector，不依赖原型二进制。
- [x] 验证 Client Bundle 可以复制到干净设备并从 Bridge GUI 运行。

## 阶段 3：公开发布

目标：让项目对普通玩家安全且易于理解。

- [x] 发布 GitHub Release 资产。
- [x] 增加 Release Please 发布流程。
- [ ] 构建带签名 Relay Server 元数据的 Directory Service。
- [ ] 增加 Client/Relay 协议最低和最高版本策略。
- [x] 编写用户文档、FAQ、Windows 安全提示和校验和说明。
- [ ] 定义公共 Relay Server 策略。
- [ ] 支持通过 Directory Service 撤销 Relay 和发布信任元数据。

## 阶段 4：UDP 投递实验与加固

目标：在不干扰 TCP 控制和 TCP/UDP 数据路径基线的前提下，探索有界 UDP 投递改进。

- [x] 增加工作量证明或同等的抗滥用门槛。
- [ ] 研究有界 UDP 重复发送/去重 Profile。
- [ ] 研究围绕完整 Relay Data Frame 的逐跳 UDP FEC。
- [ ] 在面向用户开放前测量额外带宽、恢复率、尾延迟和 Relay CPU。
- [ ] 研究 Linux 原生或 Proton 支持。

载荷加密不在当前路线图中。
