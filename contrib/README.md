# contrib

## `0001-cosmic-files-previewer-handoff.patch`

The space bar, for COSMIC. See [the README](../README.md#the-space-bar) for why
this cannot be done from `peek`'s side.

The patch makes cosmic-files call `ShowFile` on
`org.gnome.NautilusPreviewer2` when the selection is something its built-in
gallery cannot show. It is deliberately **not** a patch that mentions `peek`:
it calls the standard interface, so it works with GNOME's `sushi` and with any
other previewer that claims the name. Images and text keep the built-in
gallery exactly as they are.

Written against cosmic-files `089ad2b` (Epoch 1.8.0) and compiled there —
`cargo check` passes on the patched tree.

```sh
git clone https://github.com/pop-os/cosmic-files
cd cosmic-files
git am < ../peek/contrib/0001-cosmic-files-previewer-handoff.patch
cargo build --release
```
