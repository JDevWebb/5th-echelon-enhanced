# Microsoft's WebView2 Evergreen Bootstrapper

`MicrosoftEdgeWebview2Setup.exe` is Microsoft's small installer for the WebView2 runtime, which
the launcher's window needs on Windows (part of Windows 11 and of Edge; some Windows 10 PCs lack
it). The Windows launcher carries it and runs it, for that Windows user, when the runtime is
missing (`src/webview2.rs`); if that fails, it shows Microsoft's download page as before.
Microsoft allows apps to redistribute it ("Distribute your app and the WebView2 Runtime",
learn.microsoft.com).

It's kept here, not fetched at build time, so builds don't depend on Microsoft's link or change
when Microsoft replaces the file. Its SHA-256 is pinned in `build.rs`; a Windows build stops if
the file differs.

- Source: `https://go.microsoft.com/fwlink/p/?LinkId=2124703` (fetched 2026-11-04)
- SHA-256: `cf2f2445fef0c5c14e372045f84998771eeb8bb01d59be4afd00692fa160082e`
- Authenticode: signed by Microsoft Corporation (Microsoft Code Signing PCA 2024, Microsoft Root
  Certificate Authority 2011); checked with `osslsigncode verify -CAfile <that root>`.

To update it: fetch it again from the source, check its signature as above, and change the
SHA-256 in `build.rs` and here.
