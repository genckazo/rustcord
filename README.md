# Rustcord

A lightweight, privacy-focused Discord desktop client written in Rust, primarily for macOS and also buildable on Linux. The client supports guild text channels and DMs, live messages, sending, paginated history, and message search. The transcript shows embed cards and received attachments. Voice, file uploads, avatars, threads and rich formatting in message text are not implemented.

**Warning:** Third-party clients using Discord user tokens violate Discord's Terms of Service and may result in account restrictions or termination. This is experimental, unofficial software; use at your own risk. Never share a token: it grants access to your account.

## Build and run

The Rust toolchain is pinned in `rust-toolchain.toml` (Rune requires a recent compiler); rustup installs it automatically.

```sh
cargo run --release
```

Linux build prerequisites (Ubuntu):

```sh
sudo apt-get install build-essential pkg-config libssl-dev libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev libvulkan1 mesa-vulkan-drivers
```

Remember me uses macOS Keychain or Linux Secret Service (an unlocked service and D-Bus session are required). There is no plaintext fallback. Without Remember me, a token is kept in process memory only. Log out removes the stored credential; if the OS keychain is inaccessible, the UI reports that manual removal of the `rustcord` / `discord-token` entry is needed.

### Sign in

Discord does not provide an officially supported user-token login flow for third-party clients. If you accept the risks, open **your own** signed-in Discord web session at `https://discord.com/app`, open browser developer tools → Network, and inspect the `Authorization` request header on a Discord API request (for example a channel messages request). Paste that value into Rustcord's password-style field. Do not use another person's token, paste scripts into the console, or send the token anywhere else. Clear your clipboard afterwards. A bot token is not supported.

Select a server or Direct messages, select a text channel, and type a message; Enter sends. Scroll to the top (or use Load older messages) for history; Scroll to bottom jumps back to recent messages. Mentions are sent with automatic mention parsing disabled.

Use the search field above the conversation to search the selected server (tick **This channel** to limit it to the open channel) or the open DM. Results load 25 at a time (Load more results); click one to open its channel. Search runs on Discord's servers, so the query text is sent to Discord; results are held in memory only and cleared when you switch server or log out. If Discord is still indexing a server, Rustcord retries briefly and then asks you to try again.

### macOS notifications

The signed `Rustcord.app` can show silent macOS notifications for incoming DMs and direct server mentions when the Rustcord window is not focused. The first eligible message asks for permission. Change permission in **System Settings → Notifications → Rustcord** (System Preferences on older macOS versions). Alerts contain no sender, channel, or message text. Discord's server and channel notification preferences do not sync to Rustcord.

Notifications are unavailable with `cargo run` and on Linux. For local development, build and open the signed app bundle as described below.

## Privacy

No telemetry, analytics, tracking endpoints (`/science` included), message database, or message disk cache. Rustcord contacts only Discord REST at `discord.com` and WebSocket Gateway at `gateway.discord.gg`. HTTP redirects and environment proxies are disabled. Resume URLs are used only if they match the allowlist exactly; regional hosts fall back to the canonical gateway. Viewing messages does not load remote images or files. Clicking a link hands its URL (not your authorization token) to your browser, where normal browser history and downloads apply. Attachment links must point to Discord's CDN or media proxy and may expire. Discord itself still receives your token, IP address, and chat activity. The UI framework's local headless tools write PNGs only when explicitly invoked. OS swap, crash collection, screenshots, and clipboard history are outside the app's control.

## Architecture

One binary crate. Rune is pinned to `b7a08d6aba94dc19bba2170c460b5d9841eec11b`; its required vendor patches are pinned to the same revision. Rune replaces the original egui/glow plan and uses its own GPU renderer (wgpu / native Metal on macOS). `ui` owns the views; `network` runs Tokio on a background thread, with bounded command/event channels and a Rune waker. `gateway` owns session/resume/heartbeats, `api` owns REST and 429 retry delays, `store` owns memory-only guild/channel/message state, and `auth` exclusively accesses the OS keychain. These boundaries leave room for a future voice service without coupling audio to UI state.

Message caches retain at most 200 newest messages per channel. Older pages beyond that window are discarded; this phase does not provide unbounded historical browsing. REST work is serialized and may queue behind rate limits. Discord's undocumented user-account gateway behavior may change; no live account integration is exercised by tests.

## Checks and deterministic screenshots

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release
mkdir -p screenshots
target/release/rustcord --snap screenshots/login.png
target/release/rustcord --scene main --snap screenshots/main.png
target/release/rustcord --scene search --snap screenshots/search.png
```

Supply-chain checks (also run by `.github/workflows/supply-chain.yml` on PRs, pushes to main and weekly): `cargo audit` (config in `.cargo/audit.toml`) and `cargo deny --locked check` (config in `deny.toml`). Ignored advisories are listed with their reason in both files.

All scenes render with Rune headlessly, without reading the OS keychain or making network requests; the main scene uses fake data. A usable GPU backend is required (Mesa llvmpipe works on Linux). Never run headless capture tooling against a real logged-in session: diagnostic output may contain visible messages.

## macOS packages

On macOS with Xcode Command Line Tools and rustup installed, run:

```sh
bash scripts/bundle-macos.sh
```

This builds `dist/Rustcord.app` and `dist/Rustcord-<version>-macos-<arch>.dmg` for the installed Rust toolchain's architecture, targeting macOS 11 or newer. Mount the DMG and drag Rustcord to Applications. CI builds and checks separate packages for Apple silicon (`arm64`) and Intel (`x86_64`) and uploads each DMG alongside a ZIP of the app.

Local and CI packages are ad hoc signed and not notarized. They are for development and may be blocked by Gatekeeper when downloaded. For distribution, install a **Developer ID Application** certificate in your keychain and set `MACOS_SIGNING_IDENTITY` to its name when running the script. Then run the following with the resulting DMG path and a configured notarytool keychain profile:

```sh
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
# Continue only if notarization is accepted.
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
spctl --assess --type open --context context:primary-signature -v "$DMG"
```

Distribute the notarized DMG, not the development ZIP.
