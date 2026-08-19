# Relay 可观测性

[English](relay-observability.en.md)

Relay Server 可以通过标准 OTLP/gRPC 导出 OpenTelemetry 指标和 Trace，只有 Relay 会执行导出。

接收端可以是本地 OpenTelemetry Collector、Vector、SigNoz Collector 或其他兼容 OTLP 的组件。Tractor Beam 不依赖特定部署拓扑或可观测性后端。

## 本地日志格式

Relay 日志始终写入标准输出，并与 OTLP 导出相互独立。设置 `LOG_FORMAT=json` 可输出适合 journald、Docker 或其他结构化日志采集器读取的逐行 JSON。

`LOG_FORMAT` 只支持 `text` 和 `json`。未设置时默认为 `text`，便于直接运行时阅读。`RUST_LOG` 控制过滤；缺失或无效时默认使用 `info`。

## 配置

只有配置中存在 `[telemetry]` 时才会启用 Telemetry；环境变量无法单独启用它。

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-guangzhou-1"
```

- `otlp_endpoint` 是 OTLP/gRPC 接收端地址；当前不支持 OTLP/HTTP。
- `service_instance_id` 在同时运行的 Relay 实例之间必须稳定且唯一。

每个信号都包含以下标准 Resource Attribute：

| 属性 | 值 |
|---|---|
| `service.name` | `tractor-beam-relay` |
| `service.version` | Relay 构建版本 |
| `service.instance.id` | 配置的稳定实例 ID |

## 指标

| 名称 | 类型 | 单位 | 含义 | Attribute |
|---|---|---:|---|---|
| `tractor_beam.relay.room.active` | Gauge | `{room}` | 活动房间 | 无 |
| `tractor_beam.relay.peer.active` | Gauge | `{peer}` | 活动 Peer | `network.transport`, `peer.presence` |
| `tractor_beam.relay.connection.operation` | Counter | `{connection}` | 接受、阻止和关闭的 TCP 连接 | `outcome` |
| `tractor_beam.relay.connection.active` | UpDownCounter | `{connection}` | 当前活动的已接受 TCP 连接 | 无 |
| `tractor_beam.relay.control.operation` | Counter | `{operation}` | 控制操作结果 | `operation`, `outcome` |
| `tractor_beam.relay.control.operation.duration` | Histogram | `s` | 控制处理延迟 | `operation` |
| `tractor_beam.relay.session.establishment.duration` | Histogram | `s` | Join/Resume 建连时间与结果 | `operation`, `network.transport`, `outcome` |
| `tractor_beam.relay.data.frame` | Counter | `{frame}` | 接受、拒绝和转发的帧 | `network.transport`, `direction`, `frame.type`, `outcome` |
| `tractor_beam.relay.data.io` | Counter | `By` | 接受和转发的字节数 | 与 Data Frame 相同 |
| `tractor_beam.relay.data.dispatch.duration` | Histogram | `s` | 路由和出口发送延迟 | `network.transport`, `frame.type` |
| `tractor_beam.relay.tcp.egress.queue.max_utilization` | Gauge | `1` | 当前每 Peer TCP 队列的最高利用率 | 无 |
| `tractor_beam.relay.tcp.egress.queue.full` | Counter | `{frame}` | 因 TCP 出口队列已满而拒绝的帧 | `frame.type` |

所有时长 Histogram 使用以下秒级显式边界： `0.00025`、`0.0005`、`0.001`、`0.0025`、`0.005`、`0.01`、`0.025`、`0.05`、 `0.1`、`0.25`、`0.5`、`1` 和 `2.5`。

有界值如下：

- `network.transport`：`tcp`、`udp`
- `peer.presence`：`connected`、`reconnecting`
- `direction`：`inbound`、`outbound`
- `frame.type`：`game`、`probe`
- 队列已满时的 `frame.type`：`control`、`game`、`probe`
- 数据 `outcome`：`accepted`、`forwarded`、`duplicate`、`rate_limited`、`rejected`
- 连接 `outcome`：`accepted`、`blocked`、`closed`
- 建连 `operation`：`join`、`resume`、`unknown`
- 建连 `outcome`：`accepted`、`rejected`、`failed`、`disconnected`、`timeout`
- 控制 `operation`：`bootstrap`、`join_begin`、`join_proof`、`resume`、 `udp_path_request`、`ping`、`pong`、`stop`、`udp_path_hello`、`detach`、 `session_expire`
- 控制 `outcome`：`attempted`、`accepted`、`rejected`

## Trace

Span 名称是固定值：

| Span | 边界 | 重要的有界或关联字段 |
|---|---|---|
| `relay.session.establish` | 从一次 TCP Accept 到 Join/Resume 成功或拒绝，以及必需的 UDP 路径验证 | 进程本地 Attempt ID、`session.operation`、`network.transport`、`outcome`、`error.type` |
| `relay.bootstrap` | 兼容性 Bootstrap 子 Span | `outcome`、`error.type` |
| `relay.join.begin` | Join Challenge 子 Span | `outcome`、`error.type` |
| `relay.join.proof` | Join Proof 子 Span | `outcome`、`error.type` |
| `relay.resume` | Resume 子 Span | `outcome`、`error.type` |
| `relay.udp.validate` | 必需的 UDP 路径验证子 Span | `outcome`、`error.type` |

建连 Root Span 会在成功、拒绝、失败、断开或 15 秒 Trace 专用 Deadline 时结束。

## 容量与服务器选购

不要根据房间数、Peer 数或单次指标峰值直接升级服务器。只有 Relay 指标与主机指标持续指向同一瓶颈时，才需要调整规格。

| 现象 | 优先检查或升级 |
|---|---|
| TCP 和 UDP 发送延迟同时上升，且 CPU 持续饱和 | CPU |
| 吞吐接近网卡或服务商限制，丢包增加，但 CPU 正常 | 带宽和网络线路 |
| RSS 或内存压力随 Peer 数持续增长 | 内存 |
| 只有房间、Peer 或流量增长，没有资源压力 | 暂不升级 |

`rate_limited` 通常表示 Peer 超出配置限制，增加服务器资源不能解决。服务器位置和线路最终应通过 Client 的实际房间路径质量验证，Relay 指标不能替代玩家体验。
