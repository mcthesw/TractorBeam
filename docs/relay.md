# 部署 Relay Server

[English](relay.en.md)

可以使用 Docker 或独立可执行文件部署。两种方式都使用同一份
[`relay.toml`](../deploy/relay.toml)。
默认配置监听 `25910/TCP` 和 `25910/UDP`，OTLP 默认关闭。启动前，请在主机
防火墙以及云服务商的防火墙或安全组中放行这两个端口。

如需修改默认值，请先参阅 [Relay 配置说明](relay-configuration.md)。

## 使用 Docker 部署

将 [`docker-compose.relay.yml`](../deploy/docker-compose.relay.yml) 和
[`relay.toml`](../deploy/relay.toml) 复制到准备运行 Relay 的目录，并保持两个
文件位于同一目录，然后执行：

```sh
docker compose -f docker-compose.relay.yml up -d
```

## 使用可执行文件部署

从[最新版本](https://github.com/mcthesw/TractorBeam/releases/latest)下载服务器
平台对应的 Relay，并将 [`relay.toml`](../deploy/relay.toml) 下载到目标目录。

Linux 下先添加执行权限，再启动 Relay：

```sh
chmod +x TractorBeam-Relay-Linux-x86_64
./TractorBeam-Relay-Linux-x86_64 --config relay.toml
```

Windows 下在 PowerShell 中启动：

```powershell
.\TractorBeam-Relay-Windows-x86_64.exe --config .\relay.toml
```

配置文件可以放在其他目录，只需将实际路径传给 `--config`。


## 高级：使用 systemd 运行

如需在 Linux 上长期运行独立可执行文件，可以创建
`/etc/systemd/system/tractor-beam-relay.service`：

```ini
[Unit]
Description=Tractor Beam Relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=tractor-beam
WorkingDirectory=/opt/tractor-beam-relay
ExecStart=/opt/tractor-beam-relay/TractorBeam-Relay-Linux-x86_64 --config /opt/tractor-beam-relay/relay.toml
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

根据实际环境创建或替换服务用户和路径，然后启用服务：

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now tractor-beam-relay
sudo systemctl status tractor-beam-relay
sudo journalctl -u tractor-beam-relay -f
```

## 高级：采集可观测性数据

在配置中添加以下内容，即可将 Relay 指标和 Trace 发送到 OTLP/gRPC 接收端：

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-example-01"
```

日志采集器读取容器或 systemd 标准输出时，可以设置 `LOG_FORMAT=json`。
信号名称和采集细节参阅 [Relay 可观测性](relay-observability.md)。
