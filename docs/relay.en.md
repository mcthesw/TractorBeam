# Deploy a Relay Server

[中文](relay.md)

Use Docker or a standalone executable. Both methods use the same
[`relay.toml`](../deploy/relay.toml). The default configuration listens on
`25910/TCP` and `25910/UDP`, with OTLP disabled. Before starting the Relay,
allow both ports through the host firewall and any cloud firewall or security
group.

See [Relay configuration](relay-configuration.en.md) before changing the
defaults.

## Deploy with Docker

Copy [`docker-compose.relay.yml`](../deploy/docker-compose.relay.yml) and
[`relay.toml`](../deploy/relay.toml) into the directory where the Relay should
run. Keep both files in the same directory, then run:

```sh
docker compose -f docker-compose.relay.yml up -d
```

## Deploy with an executable

Download the Relay for the server platform from the
[latest release](https://github.com/mcthesw/TractorBeam/releases/latest), then
download [`relay.toml`](../deploy/relay.toml) into the target directory.

On Linux, make the binary executable and start it:

```sh
chmod +x TractorBeam-Relay-Linux-x86_64
./TractorBeam-Relay-Linux-x86_64 --config relay.toml
```

On Windows, start it from PowerShell:

```powershell
.\TractorBeam-Relay-Windows-x86_64.exe --config .\relay.toml
```

The configuration may be stored elsewhere when its actual path is passed to
`--config`.

## Advanced: run with systemd

For a long-running Linux executable deployment, create
`/etc/systemd/system/tractor-beam-relay.service`:

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

Create or replace the service user and paths as appropriate, then enable the
service:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now tractor-beam-relay
sudo systemctl status tractor-beam-relay
sudo journalctl -u tractor-beam-relay -f
```

## Advanced: collect observability data

Add the following configuration to send Relay metrics and traces to an
OTLP/gRPC receiver:

```toml
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-example-01"
```

Set `LOG_FORMAT=json` when a log collector reads container or systemd output.
See [Relay observability](relay-observability.en.md) for signal names and
collection details.
