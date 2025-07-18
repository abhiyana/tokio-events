---
name: Bug Report
about: Create a report to help us improve
title: '[BUG] '
labels: bug
assignees: ''
---

## Bug Description

A clear and concise description of what the bug is.

## Steps to Reproduce

1. Go to '...'
2. Create event '...'
3. Subscribe with '...'
4. See error

## Expected Behavior

A clear and concise description of what you expected to happen.

## Actual Behavior

A clear and concise description of what actually happened.

## Minimal Reproducible Example

```rust
// Please provide a minimal code example that reproduces the issue
use tokio_events::{Event, EventBus};

#[derive(Debug, Clone)]
struct TestEvent {
    // ...
}

impl Event for TestEvent {
    fn event_type() -> &'static str {
        "TestEvent"
    }
}

#[tokio::main]
async fn main() {
    // Minimal reproduction code here
}
```

## Error Messages

If applicable, add error messages or stack traces.

```
Error message here
```

## Environment

- **tokio-events version:** (e.g., 0.1.0)
- **Rust version:** (output of `rustc --version`)
- **Operating system:** (e.g., Ubuntu 20.04, Windows 10, macOS 12.0)
- **Architecture:** (e.g., x86_64, aarch64)

## Additional Context

Add any other context about the problem here.

## Possible Solution

If you have ideas on how to fix this, please describe them here.