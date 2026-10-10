# Project Overview

This document describes the **architecture** of the *example* service and
links to the [design notes](https://example.com/design "Design") and the
`config.toml` reference. See also <https://example.com/faq>.

## Features

- Fast startup with **zero** configuration
- Pluggable backends:
  - `sqlite` for local development
  - `postgres` for production
    - supports ~~replication~~ read replicas
- [x] Authentication
- [ ] Rate limiting

> **Note:** the API is not stable yet.
> > Nested quotes are rendered too.

## Configuration

| Option      | Default | Description              |
|-------------|:-------:|--------------------------|
| `port`      | 8080    | Port to listen on        |
| `log_level` | info    | One of *debug*, *info*   |
| `workers`   | 4       | Number of worker threads |

```rust
fn main() {
    println!("hello");
}
```

Setext Heading
==============

1. First step
2. Second step

---

Escaped \*asterisks\* stay visible.

## Images

A local image renders below its caption:

![Helix logo placeholder](markdown-render.png)

A remote image stays as text: ![remote](https://example.com/x.png)
