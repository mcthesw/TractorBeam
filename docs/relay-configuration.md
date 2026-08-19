# Relay 配置说明

[English](relay-configuration.en.md)

请从仓库维护的 [`relay.toml`](../deploy/relay.toml) 开始配置。未知字段或无效值会使 Relay 在启动时退出；没有明确需求时请保留默认值。

## 监听地址

```toml
[relay_server]
tcp_bind = "0.0.0.0:25910"
udp_bind = "0.0.0.0:25910"
```

- `tcp_bind` 是控制面必需的监听地址。
- `udp_bind` 启用 UDP 数据路径；只有明确需要纯 TCP Relay 时才删除它。
- 修改任一端口后，还要同步修改 Docker 端口映射，并在主机和云防火墙中放行对应的 TCP 或 UDP 端口。
- 使用 `[::]:25910` 监听 IPv6。IPv6 通配地址是否同时接受 IPv4 取决于操作系统。

## 接入与房间容量

```toml
[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256
```

- `pow_difficulty_bits` 控制加入房间时的工作量证明。公网 Relay 保持 `18`；只有可信的本地开发 Relay 才应使用 `0`。
- `max_rooms` 限制内存中的活动房间数量。

## 流量限制

```toml
[traffic_limits]
rate_limit_per_second = 5000
byte_rate_limit_per_second = 8388608
byte_rate_limit_burst = 16777216
```

这些限制按 Peer 生效。除非实际负载和主机容量证明需要调整，否则请保留默认值。

## 访问控制

```toml
[access_control]
blocked_cidrs = []
```

`blocked_cidrs` 接受 IPv4、IPv6 地址或 CIDR：

```toml
blocked_cidrs = ["203.0.113.10/32", "2001:db8::/32"]
```

## Telemetry

没有这一节时，Telemetry 保持关闭：

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-example-01"
```

接收端必须支持 OTLP/gRPC。每个运行中的 Relay 应使用稳定且唯一的 `service_instance_id`。导出信号参阅 [Relay 可观测性](relay-observability.md)。
