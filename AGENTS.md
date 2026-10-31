# Repository Guidelines

## Project Structure & Module Organization

橘记 (Orange) is a Vite + React + TypeScript frontend with a Tauri v2 Rust desktop backend.

- `src/` contains the application UI, split by feature: `workspace/`, `knowledge-base/`, `editor/`, `agent/`, `diff/`, `settings/`, and shared types/utilities in `shared/`.
- `src-tauri/src/` contains Tauri commands, local storage, agent runtime, domain types, and secure file operations.
- `public/` stores static frontend assets such as the logo.
- `docs/product/` and `docs/prototype/` hold product notes and archived prototype references.
- `docs-site/` is the public VitePress documentation site (deployed to GitHub Pages at `https://ciri667.github.io/orange/`). Keep it in sync with code: the built-in command table mirrors `src-tauri/src/builtin_commands.rs` and the tool docs mirror `src-tauri/src/agent_tools/execute.rs`. `base` must stay `"/orange/"`.
- Generated output belongs in `dist/`, `src-tauri/target/`, and `docs-site/.vitepress/dist/`; do not edit these by hand.

## Module Boundaries

New code should land in the module that already owns that concern. Do not grow the remaining large files when a domain module exists.

**Rust (`src-tauri/src/`)**

- `commands/` — Tauri IPC adapters grouped by domain (`workspace`, `knowledge`, `notes`, `documents`, `history`, `sessions`, `settings`, `skills`, `im`, `agent`, `builtin_commands`, `logs`, `browser`). Keep command names stable. Shared blocking/IO helpers live in `commands/common.rs`. Register commands from the domain module in `lib.rs` (for example `commands::notes::create_note`).
- `builtin_commands.rs` — the single registry for built-in slash commands (`id`, name, aliases, description, scopes, argument hint). Desktop `/` picker, `/help` text, and IM parsing all read from this one list, so a new command must be declared here first. `commands/builtin_commands.rs` only exposes `list_builtin_commands` / `run_builtin_command` over it. Never hand-write `/help` copy elsewhere; the `help_text_only_lists_parseable_commands` test guards that drift.
- `storage/` — SQLite, FTS, filesystem scan/write, keyring, document history, and preview extraction. Public functions stay reachable as `crate::storage::…`. New persistence code belongs in the matching submodule (`ids`, `db`, `sessions`, `config`, `memory`, `logs`, `history`, `workspace`, `files`).
- `agent_tools/` — closed-set tools (`types`, `registry`, `execute`). The closed set is `search` / `read` / `list` / `edit` / `write` / `exec` / `browser` / `task`. Keep these eight names. `exec` executes managed non-interactive commands; `browser` dispatches Chromium actions. `task` is the only orchestration tool: child tools intersect parent permissions; writer may propose file changes, readonly roles remain readonly. Children cannot call exec/browser/MCP or recursively delegate. Async operations must be dispatched by Runtime with an exact-parameter permit.
- `browser/` — embedded browser agent (`manager`, `policy`, `types`, `fetcher`, `session`, `win32_surface`, `wkwebview_surface`). The model reaches it through the dedicated `browser` tool; that branch is async pre-dispatched in `runtime/mod.rs` next to the MCP arm, never inside `AgentTool::execute`. URL policy (http/https only), read/mutate classification, and the four-level gate live in `policy.rs`; all full-confirm calls use `operation_approval.rs`; control lease and user takeover live in `manager.rs`; the fixed-version Chromium downloader (explicit user opt-in, never silent) lives in `fetcher.rs`. On Windows the panel shows the real headful Chromium window embedded as a native child surface (`win32_surface.rs`); one native window hosts all tabs. The JPEG screencast path is retained only as the macOS compatibility fallback. `wkwebview_surface.rs` is the macOS WKWebView + isolated `WKContentWorld` bridge contract — a stub whose `native_surface_available()` returns `false` until it is implemented on a macOS machine; see `docs/product/browser-native-surface.md`. Screenshots feed the panel and receipts only — never the model.
- `runtime/` — agent loop in `mod.rs`, DeepSeek DSML parsing in `dsml.rs`, in-process subagent spawn in `subagent.rs` / `subagent_personas.rs`.
- `domain/` — IPC and persistence DTOs grouped by aggregate (`knowledge`, `history`, `agent`, `session`, `settings`, `skills`, `im`, `logs`, `payloads`). Public types stay reachable as `crate::domain::…`. Prefer existing types over ad-hoc JSON maps.
- `skills/` — catalog (built-in/custom listing), builtin (package-file synchronization), install (download/copy/conflict), resources (progressive readonly access). All runtime Skill files live in `~/.orange/skills`; bundled sources in `src/shared/skills` are shared with the browser mock. Skill has no execution IPC; generic file review adapters live in `change_sets.rs`.
- `im/` — IM provider routing; Feishu-specific code stays in `im/feishu.rs`. IM built-in commands are parsed by `crate::builtin_commands`, not by a local enum; provider entry points dispatch on `descriptor.id` after auth, dedupe, and group-mention gating so commands never reach the model.
- `fs_guard.rs`, `agent_writes.rs`, `text_edit.rs` — path policy and confirmed writes. Structured Agent file tools use `file_changes.rs`: trusted persisted changes → diff/impact review (or full-auto for this call only) → strict disk hash/directory checks → per-item results, atomic text writes, no-overwrite moves, recycle-bin deletes. `permissions.rs` is the shared execution/schema policy; readonly/Plan cannot propose or apply changes. Real commands and external services use one-time operation approval and do not promise per-mutation diffs or rollback.

**Frontend (`src/`)**

- Feature folders own UI for that surface. `workspace/` orchestrates snapshot state; do not introduce a global store. File/session/Agent/review actions belong in `useEditorActions` / `useKnowledgeBaseActions` / `useSessionActions` / `useAgentTurn` / `useReviewActions`, not in `WorkspaceShell.tsx`.
- `browser/BrowserPanel.tsx` owns the embedded-browser dock (tabs, address bar, native surface or macOS screencast fallback, take-over button, and the「系统浏览器打开」escape to the system default browser). It subscribes to `browser-*` Tauri events through `shared/api/browser.ts` and mounts from `WorkspaceShell` only when `userSettings.browser.enabled` is on. On `surfaceMode === "native"` it renders `BrowserSurfaceHost`, which reports the stage bounds to Rust instead of relaying input. Browser and MCP calls are approved through the single `agent/OperationApprovalDialog.tsx`; there is no per-domain approval event.
- `shared/externalLink.tsx` is the link-routing seam: `MarkdownLink` asks the injected handler first, and only falls back to `shared/openExternalUrl.ts` (Tauri opener) when no workspace handler handles it. `WorkspaceShell` injects `useRightPanelActions.openExternalLink`, which decides via `browser/linkRouting.ts` (pure, unit-tested) and reuses `browser_create_tab` + `browser_user_navigate`; web links therefore open in the embedded browser by default, while Ctrl/Cmd clicks, `mailto:`/`tel:` and every failure path stay on the system browser.
- `shared/tauriApi.ts` is a barrel over `shared/api/` (the only IPC boundary). Browser mock fallbacks live in `shared/mock/`, not in feature components.
- The Agent composer's `/` picker lives in `agent/AgentInput.tsx` and shows built-in commands before skills in one list (`slashPickerItems`); it only renders. Command interception and dispatch stay in `useAgentTurn.handleSubmitPrompt`, which must run **before** optimistic message persistence so commands never become chat messages. Unknown `/xxx` input falls through as a normal prompt instead of erroring. Command results use the composer-local `builtinCommandNotice`, not the workspace-wide `notice` (which renders only in the left sidebar and is cleared by `beginBusy`).
- Settings sections belong under `settings/sections/`; do not add new settings UI into `WorkspaceShell.tsx`.

## Build, Test, and Development Commands

- `npm install` installs JavaScript and Tauri CLI dependencies.
- `npm run dev` starts the Vite web app, usually at `http://localhost:5173/`.
- `npm run build` runs TypeScript checks with `tsc --noEmit` and builds the frontend.
- `npm run preview` serves the production frontend build locally.
- `npm run icon:macos:generate` regenerates the macOS icon assets from the Orange illustration.
- `npm run desktop:dev` launches the Tauri desktop app in development mode.
- `npm run desktop:build` creates an ad-hoc-signed `.app` bundle for on-device verification without a certificate.
- `npm run desktop:build:dmg` creates a certificate-free local DMG for installation-flow verification.
- `npm run desktop:build:release` creates a release package and requires `ORANGE_RELEASE_SIGNING_IDENTITY`.
- `npm run rust:test` runs Rust tests via `cargo test --manifest-path src-tauri/Cargo.toml`.

## Coding Style & Naming Conventions

Use TypeScript with React function components. Keep component filenames in `PascalCase.tsx`, utility modules in `camelCase.ts`, and Rust modules in `snake_case.rs`. Prefer explicit domain types from `src/shared/types.ts` and `src-tauri/src/domain.rs` over loose object shapes. Follow existing two-space indentation in frontend files and standard `rustfmt` formatting for Rust.

Frontend styling is hybrid. Design tokens live in `src/styles/app.css` `:root` and are mapped in `src/styles/tailwind.css`. New UI should use Tailwind utilities plus `cn()` from `src/shared/cn.ts` and semantic tokens such as `bg-surface`, `text-ink`, and `bg-accent`. Action buttons use `<Button>` from `src/shared/Button.tsx`; dialogs use `ConfirmDialog` / `ModalBackdrop`. Do not import Tailwind Preflight. Keep utilities unlayered so they can override element rules such as `button { color: inherit }`. Layout chrome for the editor and the Agent panel still lives in `app.css`—do not rewrite those panels in one pass. Do not add Ant Design, MUI, or Bootstrap.

Add clear comments for functions, classes, and core variables. Comment complex branches, loops, and Tauri or filesystem calls inline. Mark incomplete work with `todo` comments.

## Testing Guidelines

Rust tests are the current automated test path; run `npm run rust:test` before changing `src-tauri/`. There is no configured frontend test runner yet, so validate React changes with `npm run build` and, when UI behavior changes, manually check both `npm run dev` and `npm run desktop:dev`.

Name future tests after the behavior under test, for example `applies_proposed_change_only_after_hash_match` or `AgentPanel.shows citations`.

## Commit & Pull Request Guidelines

This checkout does not include Git history, so use concise Conventional Commit-style messages such as `feat: add note search filters` or `fix: guard unsafe write path`. Pull requests should include a short summary, test results, linked issue or product note, and screenshots or recordings for visible UI changes. Call out storage, filesystem, or agent-write behavior explicitly because these affect local user data.
