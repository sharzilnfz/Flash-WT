# Single static binary in Rust, explicit command before watcher daemon

Version 1 ships as one dependency-free Rust binary (brew plus curl installer).
Rust chosen because mantle and cow-cli already prove mature APFS clonefile
bindings in the ecosystem. The explicit `wt create` command ships first
because it is debuggable and honest about what touches disk; the automatic
watcher daemon comes later as an opt-in, once users trust the tool.
