# Changelog

All notable changes to `tokio-events` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.2] - 2026-06-07

### Documentation
- **Comprehensive API Docs**: Conducted a repository-wide sweep to ensure 100% of public APIs (`EventBus`, `EventEnvelope`, `EventMetadata`, `DispatcherConfig`) have detailed docstrings and `# Examples`.
- **Advanced Patterns**: Documented RPC (Request-Reply), outbox network publishing, graceful shutdown paradigms, and backpressure configurations.
- **Formatting**: Applied strict `cargo fmt` across the entire codebase.

*Note: The OpenTelemetry Distributed Tracing feature is still in development on a separate branch and is NOT included in this release.*

## [0.3.1] - 2026-06-06

### Fixed
- **Dead Code Warning**: Fixed an issue where compiling without `persistence` or `remote` features would trigger a `dead_code` warning on internal methods like `dlq_tx`.
- **Clippy Warnings**: Resolved complex type warnings by introducing `EventFilterFn` type aliases.

---

## [0.3.0] - 2026-06-06 [YANKED]

*Note: This version was yanked due to a compilation warning with default features. Please use 0.3.1 instead.*

### Added
- **Global Event Bus**: Introduced `global.rs` with `set_global_bus` and `get_global_bus` to allow accessing a globally shared event bus instance across your application without passing an `Arc` everywhere.
- **Event Filtering**: Added a synchronous `filter` method to `EventHandler` that allows dropping events instantly before they consume channel capacity.
- **Advanced Docstrings**: Upgraded all crate-level docstrings to professional standards, including detailed `# Arguments`, `# Returns`, `# Errors`, and `# Examples`.

### Changed
- **Macro Name Uniqueness**: The `#[derive(Event)]` procedural macro now automatically uses `module_path!()` to ensure that identical struct names in different modules resolve to globally unique event types, preventing collisions.
- **Dependency Bumps**: Updated internal dependencies to ensure compatibility with modern tokio ecosystems.

### Fixed
- **FilteredHandler Congestion**: Hoisted filter logic from `FilteredHandler` up into `SubscriptionManager`. Filtering now happens at dispatch time, ensuring rejected events never enter the handler's MPSC channel queue, preventing congestion.
- **Scheduled Event Volatility**: Added explicit `tracing::warn!` logging when dispatching scheduled events on an in-memory `ChannelDispatcher` (without persistence), warning developers that those events will be lost upon crash.
