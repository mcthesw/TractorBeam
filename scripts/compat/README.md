# Basement Bridge Compatibility Harness

This harness runs protocol-level compatibility probes as a version matrix:
old/new compatibility client probes against old/new Relay Server binaries. The
client probe is the versioned `check_compat.py` script from each checkout. It is
intentionally independent of the GUI so release checks can keep working while
the player-facing interface evolves.

Run the current compatibility client against a built Relay Server:

```sh
uv run --project scripts/compat python scripts/compat/check_compat.py \
  --head-relay-binary target/release/basement-bridge-relay \
  --json-out .local/compat/compat-report.json
```

Run a pull-request style old/new client x old/new server matrix:

```sh
uv run --project head/scripts/compat python head/scripts/compat/check_compat.py \
  --base-client-script base/scripts/compat/check_compat.py \
  --head-client-script head/scripts/compat/check_compat.py \
  --base-relay-binary base/target/release/basement-bridge-relay \
  --head-relay-binary head/target/release/basement-bridge-relay \
  --base-client-label old-client \
  --head-client-label new-client \
  --base-label old-server \
  --head-label new-server \
  --json-out head/.local/compat/compat-report.json
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
Head/new server probe failures still return a non-zero exit code. Old server
probe failures also fail pull-request comparisons when a base binary is
provided. The JSON report uses schema version 2 and includes a `matrix` summary
plus per-case `client` and `server` fields. In GitHub Actions, the harness also
writes a Markdown matrix table to `GITHUB_STEP_SUMMARY`.
