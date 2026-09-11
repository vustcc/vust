# Project Instructions

- **Framework:** We use Vue 3, Vite, Rust, and Axum.
- **Tone:** Be concise. Don't explain basic React/Rust concepts.

## Project Structure & Module Organization

- `crates/vust`: Control service providing the web UI and communication with `vust-agent` processes.
- `crates/vust-agent`: Distributed `vust-agent` program that only talks to `vust`. Local uses Unix sockets; distributed uses HTTPS.
- `frontend`: Vue 3 + Vite console UI (`src/`); static assets in `public/`.
- `docs`: Architecture, design, and target specifications.

## Build, Test, and Development Commands

### Backend

- **API semantics:** Use HTTP methods that match intent: `GET` for reads; `POST` for creates or complex queries; `PUT`/`PATCH` for updates; `DELETE` for removals.
- For global node management routes, consistently use a plural prefix with a semantic action suffix, e.g., `/api/v1/nodes/list`.
- For single node operation routes, consistently use a singular prefix with the node ID placed upfront and the action semantic placed at the end, e.g., `/api/v1/node/{node_id}/detail`.

### Frontend

- Lint/format: `pnpm -C frontend lint`, `pnpm -C frontend format`
- Type-check/build: `pnpm -C frontend build`

## Coding Style & Naming Conventions

- Rust: Follow `rustfmt` defaults; use `snake_case` for functions/modules and `UpperCamelCase` for types.
- Rust formatting/linting: Run `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` before opening a PR.
- Naming: Prefer descriptive, domain-oriented names (e.g., `device_fingerprint`, `scan_results`).
  - **Core Domain Naming Conventions**:
    - **Master**: The central management service for the control plane (i.e., `vust`), representing the single source of truth.
    - **Local Node**: The default node registered and running locally on the master machine (identifier `'local'`). Never refer to this as a "virtual local node", "primary node", "master node", or "master control node" in code, docs, or localization strings.
    - **Node**: Any external compute node managed by the platform other than the local node. Always refer to it simply as "node" and avoid expressions like "controlled node".
- Comments: Follow `cargo doc` conventions so generated docs are clear and readable; do not add meaningless comments.
- Frontend business UI must include stable, searchable DOM markers for debugging and automation. Prefer `data-page`, `data-ui`, and `data-slot`; use `id` only when global uniqueness is semantically required.

## Documentation Guidelines

- Add Chinese documentation comments for frontend/backend functions, structs, and modules; Rust docs must use `//!` and `///` and comply with `cargo doc` conventions.
- CHANGELOG entries should be user-facing; avoid implementation details and internal refactors.
- Please use Chinese for document content (including README and design documents).

## Commit & Pull Request Guidelines

- Commit messages follow a Conventional Commits-style prefix such as `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `style:`, or `build:`.
- Keep commits scoped and descriptive; separate refactors from behavior changes when possible.

## Development Process

- If a task requires modifying more than five files, pause first and break it down into updated tasks.
- Before writing any code, please describe your proposed approach and wait for approval. If the requirements are unclear, make sure to ask clarifying questions before writing any code.
- After modifying frontend code, run `pnpm format` and `pnpm lint`.
- After modifying backend Rust code, run `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`.
- During refactor, compatibility is not required; prioritize a clean redesign.
- When a bug is caused by backend, engine, state machine, or lifecycle timing issues, do not add frontend “stopgap” patches to mask it. Fix the source of truth first, and only adjust frontend logic when the root cause is genuinely on the frontend side.

## Communication

- Please respond in chinese by default.
