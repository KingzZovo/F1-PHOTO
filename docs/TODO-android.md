# Android client — deferred TODO

This document tracks Android scope that was intentionally **not** taken to a green build in the current milestone. The intent is to revisit after the server + Web flows are fully verified end-to-end.

## Current state (commit `19f278f` baseline)

- `android/app/src/main/...` contains a Compose scaffold with:
  - Retrofit + JWT auth client (`AuthApi.kt`).
  - Camera/gallery upload screen (`UploadScreen.kt`).
  - Offline queue worker via WorkManager (`UploadWorker.kt`).
- `android/app/build.gradle.kts` is configured but **`./gradlew assembleDebug` has never been run** in this environment (no Android SDK / `local.properties`).
- The module is **not** wired into any CI job. `.github/workflows/ci.yml` builds only the Rust server + Vue web app.

## What "done" looks like

1. Android SDK 34 + NDK installed in CI runner.
2. `./gradlew assembleDebug` produces `app-debug.apk` cleanly.
3. `./gradlew testDebugUnitTest` passes (at minimum: a `LoginViewModelTest` round-trip against MockWebServer).
4. Instrumented smoke test (or manual) against a running server:
   - login → list projects → upload one photo → see it appear on Web.
5. APK published as a release artifact alongside the Linux server tarball in `release.yml`.

## Suggested order when the work resumes

1. Add an Android job to `.github/workflows/ci.yml` running on `ubuntu-latest` with `actions/setup-java@v4` (JDK 17) and `android-actions/setup-android@v3`. Cache `~/.gradle/caches`.
2. Drop a `local.properties.example` documenting `sdk.dir=` so contributors know what to fill in locally.
3. Wire `assembleDebug` + `testDebugUnitTest` into the CI job; fail the build on regression.
4. Add `assembleRelease` (signed) to `release.yml` and attach the APK to the GitHub Release.
5. Manually exercise the upload happy path against a `f1photo` server bound to a LAN IP; confirm offline queue replays after airplane-mode toggling.

## Why deferred

Getting the Android toolchain green requires a SDK install in CI and either a packaged keystore or a published-APK story; neither is on the critical path for a usable server + Web release. The server now runs end-to-end with bundled PostgreSQL and the Web SPA is embedded into the binary, so users with browsers can already use the product. Mobile capture is a strict superset on top of that.
