# Basement Bridge Compatibility Harness

This harness runs protocol-level compatibility probes against Relay Server
binaries. It is intentionally independent of the GUI so release checks can keep
working while the player-facing interface evolves.

Run against a built relay:

```sh
uv run --project scripts/compat python scripts/compat/check_compat.py \
  --head-relay-binary target/release/basement-bridge-relay \
  --json-out .local/compat/compat-report.json
```

Run a pull-request style base/head comparison:

```sh
uv run --project scripts/compat python scripts/compat/check_compat.py \
  --base-relay-binary ../base/target/release/basement-bridge-relay \
  --head-relay-binary target/release/basement-bridge-relay \
  --json-out .local/compat/compat-report.json
```

Optional previous-release evidence for manual investigation:

```sh
uv run --project scripts/compat python scripts/compat/check_compat.py \
  --head-relay-binary target/release/basement-bridge-relay \
  --previous-tag v0.1.0 \
  --github-repository mcthesw/Basement-Bridge \
  --json-out .local/compat/compat-report.json
```

Missing previous-release assets are reported as skipped cases, not as crashes.
Head relay probe failures still return a non-zero exit code. Base relay probe
failures also fail pull-request comparisons when a base binary is provided.
