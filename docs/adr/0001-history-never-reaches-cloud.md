# Meeting History never reaches the cloud, by construction

The product's defining guarantee: cloud Backends can only ever receive the Current Meeting. The boundary is closed by removing the surface, not guarding it — there is no code path that carries History to a cloud API.

## Considered options

Redaction engines, per-item consent bridges, and local distillation were all rejected: every "guarded gate" mechanism leaks, and none can be proven safe. A closed boundary is the only version an Operator can fully reason about, and it is a guarantee competitors with cloud-side history structurally cannot make.
