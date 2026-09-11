# Changelog

## [0.6.0](https://github.com/savvagent/otto-factory/compare/v0.5.0...v0.6.0) (2026-09-11)


### ⚠ BREAKING CHANGES

* **of-core:** fence complete_job/fail_job/cancel_job/renew_claim by claim generation ([#161](https://github.com/savvagent/otto-factory/issues/161))

### Features

* **web:** add a public marketing front page ([#175](https://github.com/savvagent/otto-factory/issues/175)) ([21476b7](https://github.com/savvagent/otto-factory/commit/21476b7ad3c45c1afc9988d94a890413d13e6b0b))
* **web:** add Otto CLI to the connect page's client list ([#153](https://github.com/savvagent/otto-factory/issues/153)) ([6cb3dd3](https://github.com/savvagent/otto-factory/commit/6cb3dd36adad8f5c8c3d432af3de0fd66866b5ce))


### Bug Fixes

* **of-auth:** make claim_finish's claim and ceremony consumption atomic with its writes ([#164](https://github.com/savvagent/otto-factory/issues/164)) ([546bc63](https://github.com/savvagent/otto-factory/commit/546bc6393e0926a8cb486b8d1c40046c19239707))
* **of-core:** fence complete_job/fail_job/cancel_job/renew_claim by claim generation ([#161](https://github.com/savvagent/otto-factory/issues/161)) ([c7c5192](https://github.com/savvagent/otto-factory/commit/c7c51925f46d8392a4ef604cce598ff993101a6b))

## [0.5.0](https://github.com/savvagent/otto-factory/compare/v0.4.0...v0.5.0) (2026-09-11)


### Features

* **web:** migrate the queue page to Poller, keyed on filters too ([#146](https://github.com/savvagent/otto-factory/issues/146)) ([867c10b](https://github.com/savvagent/otto-factory/commit/867c10b637a1d55a32d78c2f5da444e0007ff3e9))

## [0.4.0](https://github.com/savvagent/otto-factory/compare/v0.3.1...v0.4.0) (2026-09-11)


### Features

* **web:** show console version in footer ([#142](https://github.com/savvagent/otto-factory/issues/142)) ([4d5cedf](https://github.com/savvagent/otto-factory/commit/4d5cedf500603f7e0e33a209111d9ef774958c09))


### Bug Fixes

* **of-core:** describe both RLS outcomes in begin_unpinned's doc comment ([#141](https://github.com/savvagent/otto-factory/issues/141)) ([3d21fa3](https://github.com/savvagent/otto-factory/commit/3d21fa3e35e67b21a2c71d058447669d76dd30f3)), closes [#112](https://github.com/savvagent/otto-factory/issues/112)

## [0.3.1](https://github.com/savvagent/otto-factory/compare/v0.3.0...v0.3.1) (2026-09-10)


### Bug Fixes

* **of-auth:** make finish_registration's credential insert and audit write atomic ([#131](https://github.com/savvagent/otto-factory/issues/131)) ([692e795](https://github.com/savvagent/otto-factory/commit/692e795bc259b2d09ad64ee374b843088934490e))
* **of-core:** repeat lease-resource backfill using the documented per-org-loop pattern ([#135](https://github.com/savvagent/otto-factory/issues/135)) ([f85f086](https://github.com/savvagent/otto-factory/commit/f85f0861b1f2f969465042b48974ce375d5aaa9c))
* **of-core:** return a retriable RaceLost error when a unique-violation race is lost with no winner ([#138](https://github.com/savvagent/otto-factory/issues/138)) ([a86a807](https://github.com/savvagent/otto-factory/commit/a86a8079aa44a7452df2c881d36d7af37a6f842f))

## [0.3.0](https://github.com/savvagent/otto-factory/compare/v0.2.0...v0.3.0) (2026-09-10)


### Features

* filter jobs by agent_type in list_jobs and ready ([#79](https://github.com/savvagent/otto-factory/issues/79)) ([74d6062](https://github.com/savvagent/otto-factory/commit/74d60627a939eb12b42155496453ec564b5e4035)), closes [#66](https://github.com/savvagent/otto-factory/issues/66)


### Bug Fixes

* bound the two unbounded queries the overview poll repeats every 30s ([#81](https://github.com/savvagent/otto-factory/issues/81)) ([f32ae18](https://github.com/savvagent/otto-factory/commit/f32ae181daeeccdb306b61462bd7dbfe317bb00f)), closes [#61](https://github.com/savvagent/otto-factory/issues/61)
* **of-auth:** update passkeys.rs test for finish_registration's new signature ([f8a5f64](https://github.com/savvagent/otto-factory/commit/f8a5f64f847637dbda93c21b8c3155d0cfe542ba))
* **of-web:** validate and parse client_ip before storing it in audit_events.ip ([#122](https://github.com/savvagent/otto-factory/issues/122)) ([ac18376](https://github.com/savvagent/otto-factory/commit/ac183760c073cc720dc220c1e3fda7bec02cd07c)), closes [#110](https://github.com/savvagent/otto-factory/issues/110)
* remove four TOTP/email-era orphaned schemas from the OpenAPI doc ([#80](https://github.com/savvagent/otto-factory/issues/80)) ([659d64f](https://github.com/savvagent/otto-factory/commit/659d64fd06f2d8b5a21b9620e2a5c6990975625e)), closes [#72](https://github.com/savvagent/otto-factory/issues/72)
* TeamMember.email is nullable, matching users.email ([#78](https://github.com/savvagent/otto-factory/issues/78)) ([f525c42](https://github.com/savvagent/otto-factory/commit/f525c420483da107b37829e910817ac1992d5607)), closes [#64](https://github.com/savvagent/otto-factory/issues/64)

## [0.2.0](https://github.com/savvagent/otto-factory/compare/v0.1.0...v0.2.0) (2026-09-10)


### Features

* generalize repo leases from (repo, branch) to (repo, resource) ([#115](https://github.com/savvagent/otto-factory/issues/115)) ([6fe084e](https://github.com/savvagent/otto-factory/commit/6fe084e0c145c9d59d646e89f198a14398787db1))

## Changelog
