// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! `peek` — a QuickLook-class file previewer for COSMIC.

mod anim;
mod app;
mod config;
mod fonts;
mod keys;
mod localize;
mod previewer;
mod surface;
mod view;

/// The command, as installed.
const COMMAND: &str = env!("CARGO_BIN_NAME");

/// `--help`. The keys come from the table the key handler reads, so the two
/// cannot drift apart.
fn usage() -> String {
    format!(
        "\
{COMMAND} — preview files without opening them

Usage:
  {COMMAND} [FILE…]  Preview the files. With one file, the arrow keys
                         move through the rest of its directory.
  {COMMAND}          Toggle the last preview, or wait for a file manager.

Options:
  -h, --help             Show this message
  -V, --version          Show the version

Keys:
{keys}
The first invocation becomes a daemon and stays resident, so binding
`{COMMAND}` to a shortcut toggles rather than starting a second copy.
",
        keys = keys::help(),
    )
}

fn main() -> cosmic::iced::Result {
    let journal = init_logging();
    localize::localize();

    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{}", usage());
        return Ok(());
    }
    if arguments
        .iter()
        .any(|arg| arg == "-V" || arg == "--version")
    {
        println!("{COMMAND} {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // The overlay is a layer surface and nothing else, and a request for one
    // that the compositor does not grant is dropped without a word. So ask
    // first: a previewer that cannot show anything says why and stops, rather
    // than staying resident behind a key that does nothing.
    if surface::layer_shell_offered() == Some(false) {
        tracing::error!("{}", surface::NO_LAYER_SHELL);
        // The log may be the journal, and this was typed at a terminal.
        if journal {
            eprintln!("{COMMAND}: {}", surface::NO_LAYER_SHELL);
        }
        std::process::exit(1);
    }

    let paths = resolve(&arguments);

    let settings = cosmic::app::Settings::default()
        // The overlay is a layer-shell surface created on demand, so there is
        // no xdg-toplevel to open at startup.
        .no_main_window(true)
        // Keep running after the preview is dismissed. The daemon is what makes
        // the next preview instant, and it is also what keeps the previewer
        // service on the bus for the file manager to find.
        .is_daemon(true)
        .exit_on_close(false)
        // The surface covers the output and is mostly empty; without this the
        // compositor composites an opaque black rectangle over the screen.
        .transparent(true)
        .client_decorations(false)
        .resizable(None)
        .antialiasing(true);

    // `run_single_instance` claims the D-Bus name. A second invocation is
    // delivered to the running daemon as an activation carrying the paths,
    // which is what turns `peek` itself into the command a keybinding runs.
    cosmic::app::run_single_instance::<app::App>(settings, app::Flags::new(paths))
}

/// Set up logging. Returns whether the log is the journal rather than stderr.
///
/// The journal first, and stderr only when there is no journal to write to.
/// Most of this daemon's life is spent started by D-Bus activation or by a
/// desktop entry, and in both cases its stderr goes somewhere the user will
/// never look — which is the same as not logging at all. `journalctl --user -t
/// peek` is somewhere they can look.
fn init_logging() -> bool {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(if cfg!(debug_assertions) {
            "warn,peek=debug"
        } else {
            "warn,peek=info"
        })
    });

    if let Ok(journal) = tracing_journald::layer() {
        tracing_subscriber::registry()
            .with(journal)
            .with(filter)
            .init();
        true
    } else {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    // stderr so logs cannot be mistaken for output by anything
                    // that pipes this.
                    .with_writer(std::io::stderr),
            )
            .with(filter)
            .init();
        false
    }
}

/// Turn command-line arguments into absolute paths.
///
/// Absolute because they are about to cross a process boundary: the daemon that
/// receives them has its own working directory, and a relative path resolved
/// there would name a different file — or, more often, no file at all.
///
/// URIs are accepted as well as paths. A desktop entry with `%U` hands over
/// `file://` URIs, and being able to take either means the same argument
/// handling serves the command line and the file manager.
fn resolve(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        // Anything that looks like a flag has already been handled above;
        // silently previewing a file called `--verbose` is not worth the
        // surprise.
        .filter(|argument| !argument.starts_with('-'))
        .filter_map(|argument| previewer::resolve(argument.as_str()))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_help_names_the_installed_command() {
        let usage = usage();
        assert!(usage.starts_with("magnetar-peek — "));
        assert!(usage.contains("  magnetar-peek [FILE…]"));
        assert!(
            !usage.contains("\n  peek "),
            "the bare `peek` is another program"
        );
    }

    #[test]
    fn flags_are_not_treated_as_files() {
        assert_eq!(resolve(&["--verbose".to_owned()]), Vec::<String>::new());
    }

    #[test]
    fn missing_files_are_dropped_rather_than_forwarded() {
        assert_eq!(
            resolve(&["/nonexistent/peek/file.png".to_owned()]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn relative_paths_become_absolute() {
        let resolved = resolve(&[".".to_owned()]);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].starts_with('/'));
    }

    #[test]
    fn file_uris_are_accepted() {
        let uri = format!("file://{}", std::env::temp_dir().display());
        let resolved = resolve(&[uri]);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].starts_with('/'));
    }
}
