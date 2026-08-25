# 参与贡献 / Contributing

[English](#english)

Tractor Beam 的维护者目前可投入时间有限，但社区贡献依旧欢迎。范围清晰、验证充分的改动更容易得到审阅。

## 适合贡献的内容

- 有明确复现方式的 Bug 或兼容性修复。
- 玩家文档、部署文档、翻译和错误提示改进。
- 范围清晰、不会扩大协议或运行时复杂度的小型体验改进。
- 能减少重复代码、诊断盲区或后续维护负担的内部整理。

## 开始之前

小型修复可以直接提交 PR，并说明问题、行为变化和相关证据。大型功能或涉及协议、Relay 数据路径、Native Hook、Injector、安全边界、发布流程的变更，应先开 Issue 对齐范围、后续维护方式和验证方法。

维护者会在时间允许时查看 Issue 和 PR。暂时没有回复并不代表拒绝贡献，感谢你的耐心。

## PR 要求

- 一个 PR 只解决一个主要问题，并保持可独立构建、测试和回滚。
- 面向玩家的中英文文档与文案应保持一致。
- 代码变更至少运行相关测试；通常还应运行 `cargo fmt --all --check`、`cargo check --workspace` 和 `cargo test --workspace`。
- 不要提交凭据、玩家日志、未脱敏的 Diagnostics Bundle 或本机配置。
- 不要在未明确约定范围时顺带修改线上基础设施、Release 或无关模块。


## English

Maintainer time for Tractor Beam is limited, but community contributions remain
welcome. Focused, well-validated changes are easier to review.

### Good contribution candidates

- Bug or compatibility fixes with clear reproduction steps.
- Player documentation, deployment documentation, translations, and error-copy
  improvements.
- Small, focused user-experience improvements that do not expand protocol or
  runtime complexity.
- Internal cleanup that removes duplication, diagnostic blind spots, or future
  maintenance burden.

### Before starting

Small fixes may go directly to a pull request with the problem, behavior change,
and relevant evidence. Large features or changes to protocols, Relay data
paths, the Native Hook, Injector, security boundaries, or release workflows
should start with an Issue that aligns scope, long-term expectations, and
validation.

Maintainers will look at Issues and pull requests as time allows. A delayed
response does not mean a contribution has been rejected, and we appreciate
your patience.

### Pull request expectations

- Keep each pull request focused on one primary problem and independently
  buildable, testable, and reversible.
- Keep player-facing Chinese and English documentation or copy aligned.
- Run relevant tests for code changes; normally also run
  `cargo fmt --all --check`, `cargo check --workspace`, and
  `cargo test --workspace`.
- Do not submit credentials, player logs, unredacted Diagnostics Bundles, or
  machine-local configuration.
- Do not bundle infrastructure, Release, or unrelated module changes without an
  explicitly agreed scope.
