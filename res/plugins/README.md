# Writing a peek plugin

A plugin is one TOML file. Drop it in `~/.local/share/peek/plugins/` (or
`/usr/share/peek/plugins/` for every user) and restart `peek`.

Plugins are consulted **only for files peek has no previewer for** — the ones
that currently show the metadata card. A plugin can therefore add previews and
can never break one: installing it cannot change how your photographs, source
files, or PDFs look.

## Routing a type to a previewer peek already has

The common case is not "decode a new format", it is "this extension is really
source code" or "this MIME type is really a zip". That needs no code:

```toml
name = "Pine Script"

[[previewer]]
extensions = ["pine"]
handler = "text"
syntax = "JavaScript"   # optional; a syntect syntax name or token
```

`handler` is one of `text`, `image`, `vector`, `archive`, `media`, `command`.

## Running a program

Anything that genuinely needs a decoder declares a command. It is given the
file and writes a preview back:

```toml
name = "Blender scenes"

[[previewer]]
extensions = ["blend"]
handler = "command"
command = "blender-thumbnailer %i %o"
output = "image"        # "image" (default) or "text"
timeout = 5000          # milliseconds; capped at 30000
```

| Placeholder | Is |
| --- | --- |
| `%i` | the file being previewed |
| `%o` | a scratch path your command must write to |
| `%s` | the target size in pixels |

`output = "image"` means `%o` is any image format peek decodes; `output =
"text"` means it is UTF-8 text, highlighted with `syntax` if you name one.

Because the contract is a path in and a file out, most freedesktop
thumbnailers already work as plugins — point `command` at the same line the
`.thumbnailer` file uses.

## What to know before shipping one

- **A plugin runs a program.** It is your own executable, with your own
  privileges, exactly like a thumbnailer or a desktop entry. Nothing sandboxes
  it. File *names* are safe — the command is split into arguments before any
  path is substituted, and no shell is involved, so a file called
  `'; rm -rf ~'` is one argument and not a command — but the program you name
  can do whatever it likes.
- **Be quick.** A previewer is judged on the delay between pressing space and
  seeing the file. Past the timeout the command is killed and the metadata
  card is shown.
- **Write nothing but `%o`.** Anything else is a side effect the user did not
  ask for by hovering over a file.
- Plugins load once at startup, sorted by file name, and the user's own
  directory wins over the system one.
