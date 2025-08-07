<div align="center">

# futures-native

Platform-native async utilities

</div>

## Overview

A collection crates providing platform-native implementations of common async patterns without runtime dependencies. These crates work with any async runtime (Tokio, async-std, smol, etc.) by leveraging OS-specific APIs directly.

## Packages

### [`blockon`](blockon/)
A 50 line, 0 dependency future executor that supports nested calls to `block_on`.

### [`request-native`](request-native/)
Platform-native HTTP client with reqwest-compatible API and Tower integration. Uses NSURLSession on macOS, WinHTTP on Windows, and libcurl on Linux for optimal performance and system integration.

### [`timeout-native`](timeout-native/)
Platform-native timeout implementation using OS timers (Grand Central Dispatch on macOS, waitable timers on Windows, timerfd on Linux) instead of runtime-specific reactors.

## Features

- **Zero runtime dependencies** - Works with any async runtime
- **Platform optimized** - Leverages native OS APIs for best performance
- **Standard interfaces** - Compatible with existing Rust async ecosystem
- **Minimal overhead** - Direct OS integration without abstraction layers
