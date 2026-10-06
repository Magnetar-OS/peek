# Flatpak

The manifest builds offline, so the crate sources are generated from the
lockfile first. From the repository root:

```sh
# whenever Cargo.lock changes
just flatpak-sources

cd packaging/flatpak
flatpak-builder --user --install --force-clean build com.magnetaros.Peek.yml
```

Needs `org.freedesktop.Sdk//26.08` and
`org.freedesktop.Sdk.Extension.rust-stable//26.08`, and `flatpak-builder`.
Without a packaged `flatpak-builder`, the Flatpak of it does the same job and
fetches the SDK itself:

```sh
flatpak --user install flathub org.flatpak.Builder
flatpak run org.flatpak.Builder --user --force-clean \
    --install-deps-from=flathub build com.magnetaros.Peek.yml
```

Leave `--install` off to build without installing, and try the result from
the build directory:

```sh
flatpak-builder --run build com.magnetaros.Peek.yml magnetar-peek --help
```

`generated-sources.json` is not committed: it is megabytes of checksums that
would dominate every diff, and it is reproducible from `Cargo.lock` in one
command. It covers the libcosmic git dependency as well as the registry
crates. It, the generator script and flatpak-builder's `build` and
`.flatpak-builder` directories are in `.gitignore`.

Poppler is built as a module because the freedesktop runtime does not carry
it, and `poppler-data` with it for the encodings poppler does not build in.
Each `sha256` is the checksum of the release tarball named in `url`; when a
version moves, download the new tarball and replace both together.

The build passes `--ignore-rust-version` to cargo. The workspace asks for the
compiler its toolchain pins, and the SDK's `rust-stable` extension trails it:
1.98.0 against 1.98.1 on 2026-09-30, and still 1.98.0 on 2026-10-03, a minor
release behind the pinned 1.99.0. The flag only skips cargo's version check,
so the code still has to compile on the extension's compiler. Check that
before building, with the same release from rustup:

```sh
cargo +1.98.0 check --workspace --ignore-rust-version --locked
```

It passed on 2026-10-03. On 2026-10-06 the extension was still at 1.98.0 and
the manifest's own build — `cargo build --release --workspace`, not only a
check — succeeded on it. When it fails, the Flatpak cannot be built until the
extension reaches the pinned release.

## Known limits

- **On COSMIC the Flatpak cannot show a preview.** The overlay is a
  `wlr-layer-shell` surface, and cosmic-comp does not offer that protocol to
  a client Flatpak has sandboxed: Flatpak marks the connection with a
  security context, and the compositor withholds its privileged globals from
  every such client. Run on 2026-10-06 against cosmic-comp 1.9.0, the
  sandboxed process was offered 36 globals, `zwlr_layer_shell_v1` not among
  them, where a process outside the sandbox was offered 59 including it.
  `magnetar-peek` checks for this at start, says so, and exits with status 1
  rather than staying resident with nothing to draw on. On COSMIC, use the
  native package. The Flatpak is for compositors that offer layer-shell to
  sandboxed clients.
- The sandbox sees the host read-only, except for the host's `/usr` and
  `/etc`: inside the sandbox those paths are the runtime's. A file under
  them cannot be previewed from the Flatpak.
- Settings are shared with a native install: both read and write
  `~/.config/cosmic/com.magnetaros.Peek/`, the one host path the Flatpak may
  write to.
- Plugins are read from `~/.var/app/com.magnetaros.Peek/data/peek/plugins/`,
  not `~/.local/share/peek/plugins/`, and a plugin's command runs inside the
  sandbox, where only the runtime's programs exist.
- "Open" hands the file to the host's default application through the
  OpenURI portal. The opened application is not placed in a systemd scope of
  its own, as the native build does: the sandbox cannot reach systemd.
- The thumbnailer entry is installed inside the Flatpak, where the host's
  thumbnail factory does not look. Thumbnails in the file manager need the
  native package.
