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

`generated-sources.json` is not committed: it is megabytes of checksums that
would dominate every diff, and it is reproducible from `Cargo.lock` in one
command. It covers the libcosmic git dependency as well as the registry
crates.

Poppler is built as a module because the freedesktop runtime does not carry
it. Its `sha256` is the checksum of the release tarball named in `url`; when
the version moves, download the new tarball and replace both together.
