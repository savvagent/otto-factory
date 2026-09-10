# Changelog

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
