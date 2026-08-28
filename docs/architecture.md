# 架构

[English](architecture.en.md)

Relay OpenTelemetry 需要显式配置，并且只由 Relay 进程导出。它与 Client 本地、面向玩家的房间路径质量相互独立。信号边界参阅 [Relay 可观测性](relay-observability.md)。

Tractor Beam 刻意分离四个网络边界：

1. Native Hook 通过 Local IPC v3（`TBI3`）与 Bridge Client 交换带目标地址的游戏包。只有完成所需 Steam 数据包 Hook 安装后才会报告 Ready；它不了解 Relay、房间、联机码、重连凭据或 TCP/UDP 路径选择。
2. Bridge Client 负责会话编排、所选 Relay 端点、Session Credential、重试策略、队列与丢弃策略，以及面向用户的状态。
3. Relay Protocol v3（`TBR3`）是 Client 与 Relay 的边界。Relay 负责接入、内存房间成员关系、路径验证、转发、限制、重复数据抑制和 120 秒恢复宽限期。
4. Direct Protocol v2（`TBD2`）是在现有局域网或虚拟局域网上连接 Bridge Client 的边界。直连 Peer Path 独立负责路径验证和传输帧标识，不依赖 Relay。

## 平台进程与注入路径

Client GUI、Bridge Core 网络栈和 Injector Helper 都编译为宿主平台原生程序。Native Hook 始终是由 `isaac-ng.exe` 加载的 32 位 Windows DLL。Hook 就绪后，两种平台都通过相同的 Local IPC 与 Bridge Core 通信，Relay 和局域网数据链路不区分注入方式。

### Windows

Client 启动原生 Injector Helper。Helper 打开 `isaac-ng.exe`，写入 Native Hook 路径，并通过 `CreateRemoteThread` 调用 `LoadLibraryW`。Client 通过进程模块检查和 Hook Ready IPC 确认加载。

### Linux（Proton）

Proton 在 pressure-vessel 容器中运行 Windows 进程，宿主侧 Injector Helper 无法可靠使用 Win32 Remote Thread 路径。Client 在启动 Steam 前向游戏目录部署临时 `winmm.dll` 代理和 Proton 内置 WinMM 副本，并仅为 `isaac-ng.exe` 设置 `native,builtin` Wine DLL Override。代理加载 Native Hook，并将完整 WinMM API 转发给 Proton 内置实现。

Client 通过 `/proc/<pid>/maps` 和同一 Hook Ready IPC 确认加载。停止游戏或 Client 下次启动时，它会恢复原 DLL Override 并删除临时文件；检测到非 Tractor Beam 管理的同名文件时会拒绝覆盖。

## Relay Protocol v3

每个会话都有一条可靠的 TCP 控制连接。接入前通过有界 JSON Bootstrap 选择协议版本和能力；缺少必需能力时返回结构化兼容性拒绝。Bootstrap 后，按方向区分且大小受限的 JSON 控制帧承载 Join、Resume、成员状态、路径验证、Stop 和 Ping 消息。

游戏数据不使用 JSON，而是使用固定二进制 Data Frame，其中包含连接 ID、单调递增帧 ID、源和目标 SteamID64、面向目标的投递流 ID 与序列号，以及不透明的 Isaac 载荷。TCP Profile 的帧复用控制连接；UDP Profile 使用单独完成路径验证的 UDP 地址元组。会话运行期间所选 Profile 固定不变，不会静默回退。

Direct Protocol v2 使用相同的投递流 ID 和序列号。独立的路径本地 `frame_id` 只用于传输接收判断。Native Hook 的进程全局数据包序列只用于 Local IPC 诊断，绝不解释为面向目标的网络连续性。

受能力控制的固定二进制 Probe Frame 在同一条已选数据路径上测量 Bridge Client 之间的**房间路径质量**。目标 Client 在本地回显，数据不会进入 Native Hook 或 Isaac 数据包队列。每个 Peer 的类型化结果保留在 Bridge Core 中。应用只结合新鲜、有界的房间路径质量窗口和近期 Session Health 增量，生成当前流畅度、置信度、新鲜度与证据原因。生命周期诊断计数器仍可查看，但恢复后不会永久降低当前估计。 Relay 全局 OTel 指标绝不会参与玩家侧估计。

Bridge Core 还提供只读的输入延迟证据：当前延迟是否可用、当前流畅度快照（包括最差 Peer 路径），以及证据不完整时的明确阻断原因。这一契约不会将毫秒换算为延迟单位、推荐数值、协调玩家或写入 Hook。手动读写只允许在正在运行且 Hook IPC Ready 的 Fallback/Pure 会话中使用，写入值不会自动恢复。Bridge Client 不会将这些测量导出到可观测性后端。

Wire Contract 位于 `relay-protocol`；套接字所有权和重试策略不在其中。 `bridge-relay` 将 Wire 值映射到领域状态，`bridge-core` 将 Hook 数据包映射为 Data Frame 并负责重连编排。

## 房间与凭据

房间没有玩家可编辑名称或单独接入码。128 位 Session Credential 同时作为不可猜测的查找键和持有者接入凭据。Join Code v5 将 Relay 端点、Profile 和唯一 Session Credential 打包为不透明的复制粘贴值。秘密、连接 ID、恢复密钥和路径 Token 绝不能进入日志或诊断数据。

房间成员身份与游戏运行相互独立。“新建房间”和“导入”会在报告成功前完成接入，因此玩家可以在启动 Isaac 前看到成员与房间路径质量。游戏附着到现有房间数据面，结束游戏不必离开房间。点击“离开房间”或关闭 Client 才会结束成员身份；Relay 重启不会持久化或恢复房间状态。

## 恢复

Relay 意外断开时，本地状态变为 Reconnecting。Bridge Client 立即尝试 Resume，随后以带抖动的指数退避从 250 毫秒重试到 2 秒，最长持续 120 秒。Resume 失败后可以使用同一 Relay 和同一 Session Credential 执行完整 Join；不会切换 Relay、房间或 TCP/UDP Profile。

路径不可用期间收到的实时 Hook 数据包会被排空并计数，恢复后不会重放。Relay 保留逻辑 Peer 和重复窗口 120 秒，广播 Reconnecting/Connected 成员状态变化，并在到期后移除一次。显式 Stop 会立即移除。

## 安全边界

Relay Protocol v3 有意使用明文。Session Credential 可以抵抗随机猜测房间，但无法保护流量免受 Relay 或链路中观察者读取。V3 不包含 TLS、AEAD、PAKE、载荷 MAC、 Nonce 预留或加密扩展字段。若保密性威胁模型发生变化，需要另行设计不兼容的后续协议版本。

## 未来投递 Profile

未来可以在完整 v3 Data Frame 外增加 UDP 重复发送/去重或逐跳 UDP FEC。它们必须通过控制面协商，受 Relay 数据包与字节限制约束，并且不能改变 Native Hook 数据包语义。Issue #38 的局域网直连已经作为独立 Route Adapter 实现；Relay Protocol v3 不会预先选择该拓扑。
