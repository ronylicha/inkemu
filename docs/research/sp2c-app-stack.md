# The desktop application of SP2c: versions, and Tauri's commands and events

Scope: the versions the desktop application starts on (Tauri 2, its dialog plugin, Vite, TypeScript, Vitest,
the linter, React, pnpm), and the parts of Tauri 2's API the application uses to call the `host` crate and to
receive what the emulator produces. Date of research: 2026-10-15. Every version below was read that day from
the registry that publishes it; nothing is from memory.

## 1. Versions (read on 2026-10-15)

| Package | Version | Published | Source |
|---|---|---|---|
| `tauri` (crate) | **2.12.4** (latest 2.x; 3.0.0-alpha.5 exists, not stable) | 2026-10-14 | `https://crates.io/api/v1/crates/tauri/versions` |
| `tauri-build` | **2.7.3** | 2026-10-14 | same API |
| `tauri-plugin-dialog` | **2.8.3** | 2026-10-14 | same API |
| `@tauri-apps/cli` | **2.12.3** | 2026-10-14 | `npm view @tauri-apps/cli time` |
| `@tauri-apps/api` | **2.12.4** | | `npm view @tauri-apps/api version` |
| `@tauri-apps/plugin-dialog` | **2.8.3** | | `npm view` |
| `vite` | **8.3.4** (`node ^20.19.0 \|\| >=22.12.0`) | 2026-10-08 | `npm view vite time`, `engines` |
| `typescript` | **7.0.2** (the native compiler: the package ships per-platform binaries) | 2026-07-08 | `npm view typescript time`, `dependencies` |
| `vitest` | **5.0.4** (peer `vite ^6.4 \|\| ^7 \|\| ^8`) | | `npm view vitest peerDependencies` |
| `@biomejs/biome` | **2.5.15** | | `npm view @biomejs/biome dist-tags` |
| `react`, `react-dom` | **19.3.1** | | `npm view` |
| `@vitejs/plugin-react` | **6.1.2** (peer `vite ^8`) | | `npm view` |
| `@testing-library/react` | **16.3.3** | | `npm view` |
| `@testing-library/user-event` | **14.6.8** | | `npm view` |
| `jsdom` | **30.1.2** | | `npm view` |
| `pnpm` | **12.11.2** (the host has 10.33.0) | | `npm view pnpm version` |

Tauri 2's crates declare `rust-version = 1.90`; the repository's toolchain is 1.99.0.

**The linter.** `typescript-eslint` 8.71.2, the latest, declares the peer `typescript >=4.8.4 <6.1.0`
(`npm view typescript-eslint peerDependencies`): it cannot run on TypeScript 7, whose compiler is no longer a
JavaScript library. The application lints with **Biome**, which parses TypeScript itself and depends on no
version of the compiler, and type-checks with `tsc` 7. Keeping TypeScript 6.0.3 for the sake of ESLint would
start a new application on an older compiler than the stable one.

## 2. Calling the host from the interface (Tauri 2)

Read in the Tauri 2 guides on 2026-10-15: `https://v2.tauri.app/develop/calling-rust/` and
`https://v2.tauri.app/develop/calling-frontend/`.

- **Commands** are Rust functions marked `#[tauri::command]`, registered with
  `tauri::generate_handler!` and called from TypeScript with `invoke` (`@tauri-apps/api/core`). A command that
  returns `Result<T, E>` rejects the promise with `E` serialised; arguments are deserialised from camelCase keys.
- **Large binary results**: "To return array buffers in an optimized way, use `tauri::ipc::Response`" — the
  value is not turned into JSON. The panel image (800 × 480 bytes for the X4) goes this way, on request.
- **Async commands**: they run on the async runtime instead of the main thread; borrowed arguments
  (`State<'_, T>`, `&str`) are not allowed in them, owned types are.
- **Events** "are not designed for low latency or high throughput situations", and "event payloads are always
  JSON strings making them not suitable for bigger messages".
- **Channels** (`tauri::ipc::Channel<T>`, given to a command as an argument, `Channel` class in
  `@tauri-apps/api/core`) "are designed to be fast and deliver ordered data. They are used internally for
  streaming operations such as download progress, child process output and WebSocket messages." The guide's
  example is a serde-tagged enum (`tag = "event", content = "data"`), which the application copies for its
  updates: console text, status, panel generations, notices, the end of a run.

**Consequence.** One channel per running emulator carries ordered, small updates; the image is fetched by a
command returning a `Response` when an update says a new generation exists. No event is used.

## 3. Permissions, dialogs, Linux build

- Tauri 2 refuses every command of a plugin the configuration does not grant: capabilities live in
  `src-tauri/capabilities/*.json`. The dialog plugin's permissions are `dialog:allow-open`, `dialog:allow-save`,
  `dialog:allow-message` (its default set grants the three; `https://v2.tauri.app/plugin/dialog/`). The
  application grants open and save only.
- The content security policy is only applied when set in `tauri.conf.json`; Tauri adds hashes and nonces to
  the bundled scripts and styles (`https://v2.tauri.app/security/csp/`). The application loads nothing remote.
- The Linux build needs `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev` (`https://v2.tauri.app/start/prerequisites/`); the CI job installs
  them, and the `check` job, which does not build the application, does not.
