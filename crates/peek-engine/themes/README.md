# Syntax themes

COSMIC's own light and dark syntax palettes, so a file previewed here and the
same file opened in cosmic-edit are coloured alike.

These two `.tmTheme` files are the rendered output of
[`cosmic-syntax-theme`](https://github.com/pop-os/cosmic-syntax-theme), which
templates them from a palette TOML at build time. They are checked in rather
than depended on because that crate is published only as a git repository, and
Cargo refuses to publish a crate with a git dependency — vendoring the two files
it produces is what makes `peek-engine` publishable at all. It also drops
`handlebars`, `serde`, and `toml` from the build-dependency graph, since the
templating they did has already happened.

Regenerate by building `cosmic-syntax-theme` and copying `$OUT_DIR/*.tmTheme`.

## Licensing

`cosmic-syntax-theme` is MPL-2.0, which is GPL-compatible; the MPL's copyleft is
per-file, and these files are unmodified, so they stay MPL-2.0 rather than
taking the workspace's GPL-3.0-or-later.

Each file carries its own upstream attribution in its header comment: both
derive from [One Half](https://github.com/sonph/onehalf) by Son A. Pham, MIT.
