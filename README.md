# Rustcord

A lightweight, privacy-focused Discord desktop client written in Rust, primarily for macOS and also buildable on Linux. Supports guild text channels and DMs, live messages, sending, paginated history, message search, embed cards, received attachments, and experimental guild/DM/group-DM voice calls. File uploads, avatars, threads and rich formatting in message text are not implemented.

**Warning:** Third-party clients using Discord user tokens violate Discord's Terms of Service and may result in account restrictions or termination. This is experimental, unofficial software; use at your own risk. Never share a token: it grants access to your account.

## Build and run

The Rust toolchain is pinned in `rust-toolchain.toml` (Rune requires a recent compiler); rustup installs it automatically.

```sh
cargo run --release
```

Linux build prerequisites (Ubuntu):

```sh
sudo apt-get install build-essential cmake pkg-config libopus-dev libssl-dev libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev libvulkan1 mesa-vulkan-drivers
```

Remember me uses macOS Keychain or Linux Secret Service (an unlocked service and D-Bus session are required). There is no plaintext fallback. Without Remember me, a token is kept in process memory only. Log out removes the stored credential; if the OS keychain is inaccessible, the UI reports that manual removal of the `rustcord` / `discord-token` entry is needed.

### Sign in

Discord does not provide an officially supported user-token login flow for third-party clients. If you accept the risks, open **your own** signed-in Discord web session at `https://discord.com/app`, open browser developer tools → Network, and inspect the `Authorization` request header on a Discord API request (for example a channel messages request). Paste that value into Rustcord's password-style field. Do not use another person's token, paste scripts into the console, or send the token anywhere else. Clear your clipboard afterwards. A bot token is not supported.

Select a server or Direct messages, select a text channel, and type a message; Enter sends. Scroll to the top (or use Load older messages) for history; Scroll to bottom jumps back to recent messages. Mentions are sent with automatic mention parsing disabled.

Use the search field above the conversation to search the selected server (tick **This channel** to limit it to the open channel) or the open DM. Results load 25 at a time (Load more results); click one to open its channel. Search runs on Discord's servers, so the query text is sent to Discord; results are held in memory only and cleared when you switch server or log out. If Discord is still indexing a server, Rustcord retries briefly and then asks you to try again.

### macOS notifications

The signed `Rustcord.app` can show silent macOS notifications for incoming DMs and direct server mentions when the Rustcord window is not focused. The first eligible message asks for permission. Change permission in **System Settings → Notifications → Rustcord** (System Preferences on older macOS versions). Alerts contain no sender, channel, or message text. Discord's server and channel notification preferences do not sync to Rustcord.

Notifications are unavailable with `cargo run` and on Linux. For local development, build the app bundle as described below and sign it before opening it (for example, `codesign --force --sign - dist/Rustcord.app` for ad-hoc signing on your Mac).

## Privacy

No telemetry, analytics, tracking endpoints (`/science` included), message database, audio recordings, or message disk cache. Text uses Discord REST at `discord.com` and WebSocket Gateway at `gateway.discord.gg`. Voice additionally connects to Discord-assigned voice WebSocket hosts under `discord.media` / `discord.gg`, and their negotiated UDP media servers. HTTP redirects and environment proxies are disabled for REST. Resume URLs are used only if they match the allowlist exactly; regional hosts fall back to the canonical gateway. Viewing messages does not load remote images or files. Clicking a link hands its URL (not your authorization token) to your browser, where normal browser history and downloads apply. Attachment links must point to Discord's CDN or media proxy and may expire. Discord itself still receives your token, IP address, and chat activity. The UI framework's local headless tools write PNGs only when explicitly invoked. OS swap, crash collection, screenshots, and clipboard history are outside the app's control.

## Voice calls (experimental)

Click a 🔊 guild voice channel to join, or **Join call** in a DM/group-DM header. The bottom voice panel has mute, deafen (also mutes transmission), and disconnect controls. Voice members come from Gateway voice states; a filled green circle marks received speech. Use headphones: there is no echo cancellation, noise suppression, automatic gain control, or device picker.

Only one call can be active. Microphone and default speakers open only after connecting and close on disconnect/logout; audio is never recorded. The native worker converts default-device audio to/from 48 kHz stereo f32, with bounded 200 ms queues and silence on underrun. Unavailable devices, permission failures, and join timeouts are reported in the panel. macOS prompts for microphone permission for the bundled app (`NSMicrophoneUsageDescription`); allow it in System Settings → Privacy & Security → Microphone. Running the bare binary may require granting permission to its launching terminal. This Linux build has not been tested against physical audio devices or a live Discord account.

The DM flow follows [Discord Userdoccers' voice connection documentation](https://docs.discord.food/topics/voice-connections): Gateway opcode 4 joins a private channel, and the private channel ID is used as the voice server ID. Joining an existing call is supported; ringing recipients is intentionally omitted (no call/ring REST endpoint is invoked). Starting a new call without ringing depends on Discord's undocumented user-client behavior and is unverified. DAVE negotiation/encryption is delegated to Songbird, not reimplemented; the UI does not independently attest a call's encryption state.

## Architecture

One binary crate. Rune is pinned to `b7a08d6aba94dc19bba2170c460b5d9841eec11b`; its required vendor patches are pinned to the same revision. Rune replaces the original egui/glow plan and uses its own GPU renderer (wgpu / native Metal on macOS). `ui` owns the views; `network` runs Tokio on a background thread, with bounded command/event channels and a Rune waker. `gateway` owns session/resume/heartbeats, `api` owns REST and 429 retry delays, `store` owns memory-only guild/channel/message/voice-member state, and `auth` exclusively accesses the OS keychain. `voice` pairs voice-state and voice-server events into Songbird ConnectionInfo and owns the driver; CPAL device I/O lives on a separate worker, never the UI thread. Songbird is pinned to 0.6.1 with defaults off and only `driver`, `receive`, `rustls`, and `tungstenite`: `driver` enables `davey` directly. No Songbird gateway, Serenity, or Twilight integration is enabled.

Message caches retain at most 200 newest messages per channel. Older pages beyond that window are discarded; this phase does not provide unbounded historical browsing. REST work is serialized and may queue behind rate limits. Discord's undocumented user-account gateway behavior may change; no live account integration is exercised by tests.

Songbird 0.6.1 has a receive-only build bug: `udp_rx.rs` imports `dashmap` but its manifest enables it only with `gateway`. The vendored, pinned release changes only that feature declaration (`receive` enables `dep:dashmap`); see `vendor/songbird/RUSTCORD-PATCH.md`. No integration feature is enabled as a workaround.

## Checks and deterministic screenshots

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release
mkdir -p screenshots
target/release/rustcord --snap screenshots/login.png
target/release/rustcord --scene main --snap screenshots/main.png
target/release/rustcord --scene voice --snap screenshots/voice.png
target/release/rustcord --scene search --snap screenshots/search.png
```

All scenes render with Rune headlessly, without reading the OS keychain or making network requests; the main and voice scenes use fake data. A usable GPU backend is required (Mesa llvmpipe works on Linux). Never run headless capture tooling against a real logged-in session: diagnostic output may contain visible messages.

Demo scenes are locally interactive: sending appends an in-memory demo message, and voice joins, mute, deafen, disconnect and DM calls change simulated state without opening devices or making network requests. Changes disappear when the process exits.

Supply-chain checks (also run by `.github/workflows/supply-chain.yml` on PRs, pushes to main and weekly): `cargo audit` (config in `.cargo/audit.toml`) and `cargo deny --locked check` (config in `deny.toml`). Ignored advisories are listed with their reason in both files.

On macOS, `bash scripts/bundle-macos.sh` creates `dist/Rustcord.app` (macOS 11+). The app is unsigned and not notarized; it is a development bundle, built for the runner's architecture, not a universal binary. CI uploads a zipped bundle preserving executable permissions.
