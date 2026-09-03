# Single static binary in Rust, explicit commands as sole mechanism

Version 1 ships as one dependency-free Rust binary (brew plus curl installer).
Rust chosen because mantle and cow-cli already prove mature APFS clonefile
bindings in the ecosystem. Explicit commands (`flashwt new`, `flashwt hydrate`) are
the sole mechanism. They are debuggable, crash-safe, and honest about what
touches disk. Background sync daemons, filesystem watchers, and automatic
bidirectional synchronization are explicitly excluded.
