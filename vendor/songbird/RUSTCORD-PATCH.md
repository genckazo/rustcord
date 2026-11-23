# Songbird 0.6.1 feature fix

Source: crates.io `songbird` 0.6.1 (ISC), with its upstream LICENSE.md restored
from the v0.6.1 tag. Only src/, build.rs, Cargo.toml, and README.md are needed.

The receive build fix adds `dep:dashmap` to the `receive`
feature in Cargo.toml. The release imports DashMap/DashSet in udp_rx.rs under
`receive`, but only enables the optional dependency under `gateway`. Without
this patch a driver+receive build fails with an unresolved `dashmap` import.
This keeps all Songbird Gateway/Serenity/Twilight integrations disabled.

The `ringbuf` requirement is raised from 0.4 to 0.5.2 (API-compatible for the
async byte adapter) to fix RUSTSEC-2026-0293 rather than suppress it.
