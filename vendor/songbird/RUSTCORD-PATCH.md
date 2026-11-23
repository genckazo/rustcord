# Songbird 0.6.1 feature fix

Source: crates.io `songbird` 0.6.1 (ISC), with its upstream LICENSE.md restored
from the v0.6.1 tag. Only src/, build.rs, Cargo.toml, and README.md are needed.

The sole upstream code/manifest change is adding `dep:dashmap` to the `receive`
feature in Cargo.toml. The release imports DashMap/DashSet in udp_rx.rs under
`receive`, but only enables the optional dependency under `gateway`. Without
this patch a driver+receive build fails with an unresolved `dashmap` import.
This keeps all Songbird Gateway/Serenity/Twilight integrations disabled.
