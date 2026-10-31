Windows CI and `packaging/windows/build.ps1` place the reproducible WSL payload
and its SHA-256 checksum here:

- `splitty-wsl-x86_64.tar.gz`
- `splitty-wsl-x86_64.tar.gz.sha256`

The archive root contains the static `splitty` musl binary and the
`shell-integration/` directory. These generated files are not committed.
