# Desktop platform checks

The native process tests run on macOS, Windows, and Linux in `Platform checks`.
They build the real release executable, including Windows' GUI subsystem, then
exercise its headless OpenSSH password helper. A temporary localhost SSH server
accepts forwarding but refuses shell sessions, like a tunnel-only service account.
Tests cover Unicode and shell punctuation in passwords and executable paths,
successful forwarding, rejection of a wrong password, and missing credentials.
They never use a real Team database or the user's SSH configuration.

Run locally after installing the frontend dependencies:

```sh
npm run build
python3 -m venv /tmp/hpbar-platform-tests
/tmp/hpbar-platform-tests/bin/pip install paramiko==4.0.0
/tmp/hpbar-platform-tests/bin/python scripts/test-ssh-platform.py
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
```

Windows uses the Python executable from its virtual environment instead.
OpenSSH Client must be installed; the app prefers native Windows OpenSSH over
Git/MSYS SSH, and reports a missing client instead of silently ignoring passwords.
SSH passwords retain their exact contents and are supplied only through the SSH
child's environment. The helper writes directly to OpenSSH's private stdout pipe;
it starts no GUI and writes no password script. Local password storage is unchanged.

Popup geometry tests cover bottom/top/side taskbars, negative monitor coordinates,
DPI-scaled sizes and content growth. Unpinned popups re-anchor on resize and DPI
changes using the monitor work area; pinned windows retain their moved position.
Pinned header dragging uses Tauri's native start-dragging API on all platforms,
with buttons and other controls excluded. Wayland compositors can restrict an
application's requested absolute position; native dragging remains compositor-led.

The audit also removed Windows `cmd /C start` launching in favor of Unicode
ShellExecuteW, hid background SSH/hostname consoles, and removed delete-before-
rename gaps in Windows account and usage history updates. Installer selection,
Claude credential file paths, Unix Keychain gating and Linux tray menu access were
reviewed; intentional platform differences remain explicit.
