---
name: powershell-cli
description: Build and validate global PowerShell/CMD launchers, interactive menus, PATH installation, and terminal UX.
---

# Powershell Cli

Keep `sapiens-agent` callable from any directory through a user-scoped PATH launcher. Prefer a `.cmd` shim for PowerShell compatibility and keep the binary private to the installation. Menus must explain every action, handle cancellation and non-interactive mode, and never pause into a silent no-op. Test fresh install, repeat install, new terminal, missing release, and port/process errors.
