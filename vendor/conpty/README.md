# Microsoft ConPTY runtime

Pinned package: `Microsoft.Windows.Console.ConPTY` **1.24.260710001** (x64).

Source: https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.24.260710001/microsoft.windows.console.conpty.1.24.260710001.nupkg

The unmodified `conpty.dll` and `OpenConsole.exe` are Microsoft-signed (Authenticode verified at import). License: MIT, see `LICENSE`.

SHA-256:

- `x64/conpty.dll`: `39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8`
- `x64/OpenConsole.exe`: `b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160`

TShell embeds these files and extracts the exact bytes into its versioned user cache before opening a PTY. It never replaces Windows system components. This version fixes OSC colour-query forwarding missing in the Windows inbox ConPTY tested on build 26200. TShell loads the DLL by absolute path before crates.io `portable-pty` opens it by name; Windows reuses the loaded module. Non-Windows builds retain the upstream transport; Windows ARM64 is not bundled or verified yet.
