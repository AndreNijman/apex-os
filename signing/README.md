# signing

MOK (Machine Owner Key) and Secure Boot signing scripts for APEX-OS kernels and bootloader components live here. **PRIVATE KEYS ARE NEVER COMMITTED TO THIS REPO** — they are injected at build time from CI secrets only. This directory holds the tooling (key-enrollment helpers, sign/verify wrappers, and the public certificate used for MOK enrollment); anything matching a private-key pattern is excluded via `.gitignore`.
