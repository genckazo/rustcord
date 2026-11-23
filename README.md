# Rustcord

A lightweight, privacy-focused Discord desktop client written in Rust, primarily for macOS and also buildable on Linux. The client supports guild text channels and DMs, live messages, sending, and paginated history. The transcript shows embed cards and received attachments. Voice, file uploads, avatars, threads, search and rich formatting in message text are not implemented.

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

### macOS notifications

The signed `Rustcord.app` can show silent macOS notifications for incoming DMs and direct server mentions when the Rustcord window is not focused. The first eligible message asks for permission. Change permission in **System Settings → Notifications → Rustcord** (System Preferences on older macOS versions). Alerts contain no sender, channel, or message text. Discord's server and channel notification preferences do not sync to Rustcord.

Notifications are unavailable with `cargo run` and on Linux. For local development, build the app bundle as described below and sign it before opening it (for example, `codesign --force --sign - dist/Rustcord.app` for ad-hoc signing on your Mac).

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
```

Both scenes render with Rune headlessly, without reading the OS keychain or making network requests; the main scene uses fake data. A usable GPU backend is required (Mesa llvmpipe works on Linux). Never run headless capture tooling against a real logged-in session: diagnostic output may contain visible messages.

On macOS, `bash scripts/bundle-macos.sh` creates `dist/Rustcord.app` (macOS 11+) containing a universal (arm64 + x86_64) release binary and prints the release binary sizes; `--native` builds only the host architecture for a faster local build. The app is unsigned and not notarized; it is a development bundle. CI uploads a zipped bundle preserving executable permissions and shows the size table in the job summary.
