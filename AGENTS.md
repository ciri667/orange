# Repository Guidelines

## Project Structure & Module Organization

橘记 (Orange) is a Vite + React + TypeScript frontend with a Tauri v2 Rust desktop backend.

- `src/` contains the application UI, split by feature: `workspace/`, `knowledge-base/`, `editor/`, `agent/`, `diff/`, `settings/`, and shared types/utilities in `shared/`.
- `src-tauri/src/` contains Rust commands, local storage, agent runtime, domain types, and secure file operations.
- `public/` stores static frontend assets such as the logo.
- `docs/product/` and `docs/prototype/` hold product notes and archived prototype references.
- Generated output belongs in `dist/` and `src-tauri/target/`; do not edit these by hand.

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

Add clear comments for functions, classes, and core variables. Comment complex branches, loops, and Tauri or filesystem calls inline. Mark incomplete work with `todo` comments.

## Testing Guidelines

Rust tests are the current automated test path; run `npm run rust:test` before changing `src-tauri/`. There is no configured frontend test runner yet, so validate React changes with `npm run build` and, when UI behavior changes, manually check both `npm run dev` and `npm run desktop:dev`.

Name future tests after the behavior under test, for example `applies_proposed_change_only_after_hash_match` or `AgentPanel.shows citations`.

## Commit & Pull Request Guidelines

This checkout does not include Git history, so use concise Conventional Commit-style messages such as `feat: add note search filters` or `fix: guard unsafe write path`. Pull requests should include a short summary, test results, linked issue or product note, and screenshots or recordings for visible UI changes. Call out storage, filesystem, or agent-write behavior explicitly because these affect local user data.
