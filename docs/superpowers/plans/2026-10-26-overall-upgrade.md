# Umidl 1.9.0 Overall Upgrade Implementation Plan

> **For agentic workers:** Use test-driven development for behavior changes and independent, scoped parallel workers. Verify the combined application before copying changes back to the original project.

**Goal:** Upgrade the whole desktop application while keeping its existing download, conversion, subtitle, appearance, and plugin capabilities; prioritize reliable conversion and simpler everyday controls.

**Architecture:** Retain Vue 3, Pinia, Naive UI and Tauri/Rust. Build in a source snapshot because the original repository has no tracked source baseline. Keep the original-source.zip backup, verify the upgrade in the snapshot, then apply only changed files to the original path.

**Tech Stack:** Existing locked Vue/TypeScript/Tauri/Rust dependencies and managed local conversion tools.

**Spec:** User's overall upgrade request expands the conversion proposal at ../../../../../outputs/umi-downloader-upgrade-proposal.md. This document records the chosen implementation details.

## Global Constraints

- Preserve user data, configured tools, appearance modes, existing task lists and all four languages.
- Do not replace the framework or add runtime dependencies without a concrete need.
- New copy uses Chinese-key translations in all four locale files. Workers supply new copy to the integrator, who owns locales.
- No publish, external messages, or installation over the user's current application.
- Version all package/application manifests consistently as 1.9.0 at delivery.
- Back up original source; only deliver verified modifications to D:/AI/Hermes GD/umi-downloader.

## Review Focus

1. Fast input changes must not show stale file or URL results.
2. Failed or partially missing startup services must not strand the startup overlay or erase existing task state.
3. Document and image targets must depend on actual input/output routes and engine availability, including PDF.
4. Manual conversion settings must not be overwritten by automatic mode or incompatible stream copy.
5. Existing model/plugin installation actions must keep their state accurate after asynchronous operations.

## Task 1: Application startup and shared state

Owner: platform worker. Files: src/stores/tasks.ts, src/stores/settings.ts, their tests; src/App.vue only if needed for startup error handling.

- [ ] Reproduce partial startup/listener failures and overlapping settings load/save issues with focused tests.
- [ ] Make startup recoverable and single-flight; preserve task updates while loading; keep successful queues when another fails.
- [ ] Ensure settings load coalesces and saves do not silently overwrite intervening user edits.
- [ ] Run relevant tests and report exact outcomes and modified files.

## Task 2: Download and subtitle reliability

Owner: workflow worker. Files: src/views/Download.vue, src/views/Subtitle.vue and new/related tests only.

- [ ] Reproduce stale download probe results, duplicate submit, page leave listener issues and subtitle start/model issues actually present.
- [ ] Snapshot requests, ignore obsolete async results, clean up page subscriptions, respect existing settings defaults.
- [ ] Make output location directly selectable where needed using existing dialog support; preserve queue behavior.
- [ ] Run relevant behavioral tests; report new translation keys to integrator without editing locale files.

## Task 3: Home, settings and plugin usability

Owner: navigation worker. Files: src/views/Home.vue, src/views/Settings.vue, src/components/SettingsPlugins.vue and relevant tests/new helper files only.

- [ ] Add clear Home shortcuts to download, conversion and subtitles; active summary should lead to the module that has active work.
- [ ] Inspect existing settings/plugin UI; implement targeted clarity and consistency improvements (search/filter for plugins, clear pending states where justified), retaining all capabilities.
- [ ] Use existing design tokens and icons, preserve card budgets and compact queues.
- [ ] Run relevant tests, report new translation keys to integrator without touching locale files.

## Task 4: Conversion correctness and usability

Owner: integrator. Files: src/views/Converter.vue, src/services/conversion.ts and tests (new), docEngine.ts, Rust docs/converter/lib/db as needed; locales owned by integrator.

- [ ] Write failing tests for source-kind and target-route filtering, including native/external documents, PDF, image and audio.
- [ ] Expose source-specific conversion capability from backend using existing route definitions. Keep desktop failures distinct from browser preview.
- [ ] Correct image defaults and document recognition; filter targets by source, show common formats first with more/search available.
- [ ] Make output folder visible/selectable, guard probing and submission, release drag listeners, prevent automatic mode overriding explicit choices.
- [ ] Recover original conversion request on retry, if database already persists it.
- [ ] Verify conversion with focused tests and real files.

## Task 5: Integration, versioning and release

- [ ] Integrate locale keys and review worker modifications.
- [ ] Run frontend tests/typecheck/build, Rust library tests, translation validation; fix meaningful failures.
- [ ] Fresh independent code review of combined changes; verify actionable fixes.
- [ ] Build Windows installer and smoke-test the app and representative local conversion routes.
- [ ] Copy verified source changes to original project, preserve backup, deliver installer and Chinese release notes in outputs/.

## Execution ledger

- Baseline: 23 frontend test files / 185 tests passed on 2026-10-26.
- Original project has all source files untracked and no usable tracked baseline. Source copy and ZIP backup replace worktree-based isolation; original remains unchanged until integration.
- Existing node_modules are reused by a directory junction; no new runtime dependencies are planned.
- User deferred platform compression. User then explicitly requested integrated simpler UI with additions/subtractions: implemented source-first conversion, purpose selection, one-click local advanced options across all three workflows, responsive navigation, reduced duplicate UI and simple-mode decoration.
- Tasks 1–4 implemented. Independent review's four Important findings have regression fixes; real local engine testing additionally exposed and fixed ICO payload/container mismatch.
- Frontend final behavior suite: 31 files / 268 tests pass; TypeScript passes. Backend final serial run: 205 passed / 7 ignored. Parallel run had one existing process-timeout timing failure; serial unchanged assertions passed. Actual DTS/ALAC/PNM/ICO matrix passes with correct output magic (ICO 00 00 01 00). Real layered PSD→PNG and empty-output rejection also verified.
