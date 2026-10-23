ARA 0.0.1-alpha - WINDOWS TESTER BUILD

Requirements: Windows 10/11 x64, 16 GB RAM, Vulkan 1.3 GPU and current driver.
Ray tracing recommended: RTX 20-series or newer / RX 6000+.
Install the x64 .NET 10 SDK (includes the runtime):
https://dotnet.microsoft.com/en-us/download/dotnet/10.0

Run the setup EXE, or extract the portable ZIP and run:
  powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
Silent install / custom destination:
  ara-0.0.1-alpha-windows-x64-setup.exe --silent --destination C:\path\Ara
Each version installs to %LOCALAPPDATA%\Programs\Ara\<version> by default.
An existing destination is never overwritten. Start menu shortcuts are versioned.
Restart your terminal after installation to pick up the per-user PATH entry.

  ara --version
  ara doctor
  ara init C:\path\MyGame
  ara-editor C:\path\MyGame --sample
  ara hub add C:\path\MyGame
  ara hub install-mcp --client claude
  ara hub install-mcp --client codex
The last two commands print client configuration; follow their output once.

The Hub UI slot is ara-hub.exe. If not included yet, the Ara Hub shortcut explains
the slot and lists registered projects. The service and global MCP already work.
The player is started by the editor or Engine Protocol; do not run it directly.
templates/starter comes from ara init. Samples and assets.lock.json are included
when present at packaging time. Follow each sample's asset setup instructions.

Bundled: CLI, editor, player, import/nav/terrain workers, shader cook, cooked shader
resources, Ara's .NET host/runtime/compile server/analyzers and agent run worker,
brand icons, open-source Slang/DXC/SPIR-V tools and original third-party notices.
The installed tools do not need a source checkout, Rust or the Vulkan SDK.
Not bundled: Microsoft .NET SDK/runtime/reference packs, MSVC tools, GPU drivers,
Steam Audio optional runtime, symbols. No proprietary Microsoft SDK binaries are
redistributed. Open-source Roslyn/NuGet libraries carry their original licenses.

Local editor/Hub crash reports: %LOCALAPPDATA%\Ara\crashes. Never uploaded.
The next interactive editor launch offers to open new reports. Keep the private
symbols ZIP for this exact build to symbolize native reports with ara-crash.
This alpha is unsigned unless the release owner supplied a signing certificate.

Uninstall from Windows Installed apps or run uninstall.cmd in the installation.
Close projects and editor/player windows first. Projects and user settings remain.
Read RELEASE_NOTES.md beside the download and docs/KNOWN_ISSUES.md in the repo.
