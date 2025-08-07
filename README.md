<div align="center">

# futures-native

Platform-native async utilities

*⚠️ Initial version - not tested and incomplete*

</div>

## Overview

A collection crates providing platform-native implementations of common async patterns without runtime dependencies. These crates work without any specific async runtime by leveraging OS-specific APIs directly.

## Packages

### [`blockon`](blockon/)
A 50 line, 0 dependency future executor that supports nested calls to `block_on`.

### [`request-native`](request-native/)
Platform-native HTTP client with reqwest-compatible API and Tower integration. Uses NSURLSession on macOS, WinHTTP on Windows, and libcurl on Linux for optimal performance and system integration.

### [`schedule-native`](schedule-native/)
Platform-native task scheduler using OS-specific APIs (Grand Central Dispatch on macOS, Windows Thread Pool API on Windows, optimized thread pools on Linux) with priority support.

### [`timeout-native`](timeout-native/)
Platform-native timeout implementation using OS timers (Grand Central Dispatch on macOS, waitable timers on Windows, timerfd on Linux) instead of runtime-specific reactors.

## Features

- **No specific runtime dependencies** - Works with any async runtime
- **Platform optimized** - Leverages native OS APIs for best performance
- **Standard interfaces** - Compatible with existing Rust async ecosystem
- **Minimal overhead** - Direct OS integration without abstraction layers
