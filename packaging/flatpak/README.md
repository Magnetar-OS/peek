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
compiler its toolchain pins, and the SDK's `rust-stable` extension can be a
patch release behind it (1.98.0 against 1.98.1 on 2026-09-30).

## Known limits

- The sandbox sees the host read-only, except for the host's `/usr` and
  `/etc`: inside the sandbox those paths are the runtime's. A file under
  them cannot be previewed from the Flatpak.
- The thumbnailer entry is installed inside the Flatpak, where the host's
  thumbnail factory does not look. Thumbnails in the file manager need the
  native package.
