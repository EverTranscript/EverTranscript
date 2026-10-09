# 05: A request with bad params gets `-32602` and the connection survives

Status: ready-for-agent

Found by a preservation review during the test prune of 2026-10-09. This gap
existed before the prune.

`server.rs:4254`–`:4258` maps every decode error except an unknown method to
`error_codes::INVALID_PARAMS` (-32602). No test sends a known method with bad
params. `tests/protocol_contract.rs:124`
(`unknown_methods_are_reported_not_fatal`) covers only `METHOD_NOT_FOUND`.

## Do

- Add a sibling test in `tests/protocol_contract.rs`: after `initialize`, send
  `meeting/get` with params of the wrong shape (for example `{"id": 5}`).
- Assert that the error contains `INVALID_PARAMS`, and that `client.status()`
  still answers `Idle` afterwards, as the unknown-method test does.

## Comments
