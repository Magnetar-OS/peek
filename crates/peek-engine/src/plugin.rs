// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Third-party previewers, declared in TOML.
//!
//! A previewer's long tail is unbounded — every niche format someone cares
//! about — and shipping a decoder for each of them in this crate is not a
//! plan. So the file types `peek` does not know about are open to plugins.
//!
//! ## Two tiers, because the long tail is two problems
//!
//! Most "add support for X" requests are not "decode a new format"; they are
//! "this extension is source code" or "this MIME is really a zip". Those need
//! no code at all, so a **declarative** rule routes the type to a previewer
//! that already exists:
//!
//! ```toml
//! name = "D source"
//!
//! [[previewer]]
//! extensions = ["d", "di"]
//! handler = "text"
//! syntax = "D"
//! ```
//!
//! The rest genuinely need a decoder, and it will not be written in this
//! crate. Those declare a **command**, which is run with the file and writes
//! a PNG or a text file back:
//!
//! ```toml
//! name = "Blender scenes"
//!
//! [[previewer]]
//! extensions = ["blend"]
//! handler = "command"
//! command = "blender-thumbnailer %i %o"
//! output = "image"
//! ```
//!
//! A subprocess rather than a dynamic library: Rust has no stable ABI, so a
//! `.so` plugin would have to be rebuilt against every release of this crate,
//! and a crash in one would take the daemon with it. A subprocess can be
//! written in any language, is killed when it takes too long, and cannot
//! corrupt the previewer's own memory. It is also exactly the contract
//! freedesktop thumbnailers already use, which means the plugins mostly exist
//! already.
//!
//! ## Plugins never override a built-in previewer
//!
//! A rule is consulted **only** when detection has otherwise concluded
//! [`Kind::Other`] — that is, only for files that would show the metadata
//! card. A plugin can therefore add previews and can never remove or corrupt
//! one, which means installing a plugin cannot regress anything and the
//! content-first detection rule stays intact: a PNG is a PNG whatever a
//! plugin claims about `.png`.
//!
//! ## Plugins run code
//!
//! A command plugin is an executable the user installed, run with the user's
//! own privileges — the same trust as a thumbnailer or a desktop entry.
//! Nothing here sandboxes it. What is guaranteed is that a *file name* can
//! never become a command: the exec line is split into arguments once, and
//! placeholders are substituted as whole argument values, never re-split and
//! never passed through a shell.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::kind::Kind;

/// How long a command plugin may run before it is killed.
///
/// A previewer is judged on the delay between pressing space and seeing the
/// file. A plugin that has not answered in this long has already lost that
/// race, and the metadata card is a better outcome than a frozen panel.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Ceiling on a plugin's declared timeout.
const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest output a command plugin may produce.
///
/// The output is decoded as an image or read as text; both paths are bounded
/// downstream, but reading a plugin's runaway output into memory to find out
/// is the stall this avoids.
const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

/// Which previewer a plugin rule routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Handler {
    /// Show the file as text, optionally naming its syntax.
    Text,
    /// Decode the file as a raster image.
    Image,
    /// Render the file as an SVG.
    Vector,
    /// List the file as an archive.
    Archive,
    /// Probe the file with GStreamer.
    Media,
    /// Run [`Rule::command`] and show what it writes.
    Command,
}

/// What a command plugin writes to its output path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// An image file, in any format the raster decoder reads.
    #[default]
    Image,
    /// UTF-8 text.
    Text,
}

/// One previewer a plugin declares.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Lower-cased extensions this rule claims, without the dot.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Exact MIME types this rule claims.
    #[serde(default)]
    pub mime_types: Vec<String>,
    pub handler: Handler,
    /// Syntax name for a text handler — "D", "Markdown". Matched against
    /// syntect's syntax set by name; ignored when it names nothing.
    #[serde(default)]
    pub syntax: Option<String>,
    /// The command line, for [`Handler::Command`]. `%i` is the input path,
    /// `%o` the output path, and `%s` the target size in pixels.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub output: Output,
    /// Milliseconds before the command is killed.
    #[serde(default)]
    pub timeout: Option<u64>,
}

impl Rule {
    /// Whether this rule claims a file.
    #[must_use]
    pub fn matches(&self, mime: &str, path: &Path) -> bool {
        if self.mime_types.iter().any(|candidate| candidate == mime) {
            return true;
        }
        crate::kind::extension(path).is_some_and(|extension| {
            self.extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
        })
    }

    /// The kind this rule's handler produces, for the frontend's layout.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self.handler {
            Handler::Text => Kind::Text,
            Handler::Image | Handler::Vector => Kind::Image,
            Handler::Archive => Kind::Archive,
            Handler::Media => Kind::Video,
            // The output decides, and it is not known until the command has
            // run. Image is the layout that suits both a rendered preview and
            // a fallback card better than a reading column would.
            Handler::Command => match self.output {
                Output::Image => Kind::Image,
                Output::Text => Kind::Text,
            },
        }
    }

    fn timeout(&self) -> Duration {
        self.timeout
            .map_or(DEFAULT_TIMEOUT, Duration::from_millis)
            .min(MAX_TIMEOUT)
    }
}

/// One plugin file.
#[derive(Debug, Clone, Deserialize)]
pub struct Plugin {
    pub name: String,
    #[serde(default, rename = "previewer")]
    pub rules: Vec<Rule>,
    /// Where it was loaded from. Not read from the file.
    #[serde(skip)]
    pub source: PathBuf,
}

/// Every plugin found on the system.
#[derive(Debug, Default)]
pub struct Registry {
    plugins: Vec<Plugin>,
}

impl Registry {
    /// Parse one plugin from TOML.
    ///
    /// # Errors
    ///
    /// Fails when the document is not valid TOML or does not describe a
    /// plugin.
    pub fn parse(toml: &str) -> Result<Plugin, toml::de::Error> {
        toml::from_str(toml)
    }

    /// Build a registry from already-parsed plugins, for tests.
    #[must_use]
    pub fn from_plugins(plugins: Vec<Plugin>) -> Self {
        Self { plugins }
    }

    /// The first rule claiming a file.
    ///
    /// First match wins, and the user's own plugin directory is scanned
    /// first, so a plugin in `~/.local/share` overrides one from a package.
    #[must_use]
    pub fn find(&self, mime: &str, path: &Path) -> Option<&Rule> {
        self.plugins
            .iter()
            .flat_map(|plugin| &plugin.rules)
            .find(|rule| rule.matches(mime, path))
    }

    #[must_use]
    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }
}

/// Directories plugins are read from, most specific first.
///
/// `PEEK_PLUGIN_DIR` overrides the search entirely, which is what the tests
/// and `probe` use to exercise a plugin without installing it.
fn plugin_dirs() -> Vec<PathBuf> {
    if let Some(override_dir) = std::env::var_os("PEEK_PLUGIN_DIR") {
        return std::env::split_paths(&override_dir).collect();
    }

    let mut dirs = Vec::new();
    if let Some(data_home) = dirs::data_dir() {
        dirs.push(data_home.join("peek/plugins"));
    }

    // `XDG_DATA_DIRS` is read directly: the `dirs` crate exposes the home but
    // not the system search path, and the system path is where a packaged
    // plugin lands.
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    for entry in system.split(':').filter(|entry| !entry.is_empty()) {
        dirs.push(PathBuf::from(entry).join("peek/plugins"));
    }

    dirs
}

/// Load every plugin on the system, once.
///
/// Scanned once per process rather than per preview: a resident daemon would
/// otherwise stat a handful of directories on every keypress. A plugin
/// installed while `peek` is running is picked up the next time it starts,
/// the same as a desktop entry.
#[must_use]
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut plugins = Vec::new();

        for dir in plugin_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };

            // Sorted, so the load order is the same on every machine rather
            // than whatever order the filesystem happens to return.
            let mut paths: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
                })
                .collect();
            paths.sort();

            for path in paths {
                match std::fs::read_to_string(&path).map(|text| Registry::parse(&text)) {
                    Ok(Ok(mut plugin)) => {
                        plugin.source = path;
                        tracing::debug!(name = %plugin.name, rules = plugin.rules.len(), "loaded a plugin");
                        plugins.push(plugin);
                    }
                    Ok(Err(error)) => {
                        // A malformed plugin is skipped rather than fatal: a
                        // previewer that refuses to start because of a third
                        // party's typo is worse than one that ignores it.
                        tracing::warn!(path = %path.display(), %error, "ignoring a malformed plugin");
                    }
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "could not read a plugin");
                    }
                }
            }
        }

        Registry { plugins }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the plugin declares no command to run")]
    NoCommand,
    #[error("the plugin's command line is empty")]
    EmptyCommand,
    #[error("could not run {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the plugin did not finish within {0:?}")]
    TimedOut(Duration),
    #[error("the plugin exited with {0}")]
    Failed(std::process::ExitStatus),
    #[error("the plugin wrote no output")]
    NoOutput,
    #[error("the plugin wrote {size} bytes, which is more than can be shown")]
    OutputTooLarge { size: u64 },
    #[error("could not read the plugin's output: {0}")]
    Io(#[from] std::io::Error),
}

/// Run a command plugin and return the path it wrote to.
///
/// The returned [`Product`] deletes the output when dropped, so a plugin's
/// scratch file does not outlive the preview that asked for it.
///
/// # Errors
///
/// Fails when the command cannot be built or spawned, does not finish inside
/// its timeout, exits non-zero, or writes nothing.
pub fn run(rule: &Rule, path: &Path, size: u32) -> Result<Product, Error> {
    let command = rule.command.as_deref().ok_or(Error::NoCommand)?;
    let output = Product::new(rule.output);

    let mut argv = split_arguments(command);
    if argv.is_empty() {
        return Err(Error::EmptyCommand);
    }

    // Substituted as whole arguments. A file name is data and must never be
    // able to become another argument, let alone another command.
    for argument in &mut argv {
        *argument = substitute(argument, path, &output.path, size);
    }

    let program = argv.remove(0);
    let mut child = std::process::Command::new(&program)
        .args(&argv)
        // stdout and stderr go to the journal with the daemon's own, which is
        // where someone debugging a plugin will look for them.
        .spawn()
        .map_err(|source| Error::Spawn {
            program: program.clone(),
            source,
        })?;

    let timeout = rule.timeout();
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                // Killed rather than left running: a plugin that hangs on one
                // file would otherwise hang on every file the user arrows
                // past, and the daemon never exits to clean them up.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::TimedOut(timeout));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    if !status.success() {
        return Err(Error::Failed(status));
    }

    let written = std::fs::metadata(&output.path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if written == 0 {
        return Err(Error::NoOutput);
    }
    if written > MAX_OUTPUT_BYTES {
        return Err(Error::OutputTooLarge { size: written });
    }

    Ok(output)
}

/// A plugin's output file, removed when it goes out of scope.
#[derive(Debug)]
pub struct Product {
    path: PathBuf,
    pub output: Output,
}

impl Product {
    fn new(output: Output) -> Self {
        // The runtime directory when there is one: it is user-private and
        // cleaned up at logout, neither of which `/tmp` guarantees.
        let directory = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        let unique = format!(
            "peek-plugin-{}-{}",
            std::process::id(),
            // Monotonic within the process, so two previews in the same
            // nanosecond cannot collide on one file.
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        Self {
            path: directory.join(unique),
            output,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Product {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Replace the placeholders in one argument.
fn substitute(argument: &str, input: &Path, output: &Path, size: u32) -> String {
    argument
        .replace("%i", &input.display().to_string())
        .replace("%o", &output.display().to_string())
        .replace("%s", &size.to_string())
}

/// Split a command line into arguments, honouring quotes.
///
/// A small shell-like splitter rather than a shell: the point is that this
/// runs *before* any placeholder is substituted, so nothing a file name
/// contains can change the shape of the command.
fn split_arguments(line: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;

    for character in line.chars() {
        match (quote, character) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(character);
                // An empty quoted string is still an argument.
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }

    if started || !current.is_empty() {
        arguments.push(current);
    }

    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECLARATIVE: &str = r#"
        name = "D source"

        [[previewer]]
        extensions = ["d", "di"]
        mime_types = ["text/x-dsrc"]
        handler = "text"
        syntax = "D"
    "#;

    #[test]
    fn a_declarative_plugin_routes_an_extension_to_a_previewer() {
        let plugin = Registry::parse(DECLARATIVE).expect("parses");
        assert_eq!(plugin.name, "D source");

        let registry = Registry::from_plugins(vec![plugin]);
        let rule = registry
            .find("application/octet-stream", Path::new("main.d"))
            .expect("the extension is claimed");

        assert_eq!(rule.handler, Handler::Text);
        assert_eq!(rule.syntax.as_deref(), Some("D"));
        assert_eq!(rule.kind(), Kind::Text);
    }

    #[test]
    fn a_rule_matches_its_mime_type_whatever_the_name() {
        let registry = Registry::from_plugins(vec![Registry::parse(DECLARATIVE).expect("parses")]);
        assert!(
            registry
                .find("text/x-dsrc", Path::new("no-extension"))
                .is_some()
        );
    }

    #[test]
    fn extensions_match_regardless_of_case() {
        let registry = Registry::from_plugins(vec![Registry::parse(DECLARATIVE).expect("parses")]);
        assert!(registry.find("x/y", Path::new("MAIN.D")).is_some());
    }

    #[test]
    fn an_unclaimed_file_finds_no_rule() {
        let registry = Registry::from_plugins(vec![Registry::parse(DECLARATIVE).expect("parses")]);
        assert!(registry.find("x/y", Path::new("photo.png")).is_none());
    }

    #[test]
    fn a_command_plugin_parses_with_its_defaults() {
        let plugin = Registry::parse(
            r#"
            name = "Blender"

            [[previewer]]
            extensions = ["blend"]
            handler = "command"
            command = "blender-thumbnailer %i %o"
            "#,
        )
        .expect("parses");

        let rule = &plugin.rules[0];
        assert_eq!(rule.handler, Handler::Command);
        assert_eq!(rule.output, Output::Image, "image is the default output");
        assert_eq!(rule.timeout(), DEFAULT_TIMEOUT);
        assert_eq!(rule.kind(), Kind::Image);
    }

    #[test]
    fn an_outlandish_timeout_is_capped() {
        let plugin = Registry::parse(
            r#"
            name = "Slow"
            [[previewer]]
            extensions = ["slow"]
            handler = "command"
            command = "sleep 999"
            timeout = 600000
            "#,
        )
        .expect("parses");
        assert_eq!(plugin.rules[0].timeout(), MAX_TIMEOUT);
    }

    #[test]
    fn a_malformed_plugin_is_an_error_rather_than_a_panic() {
        assert!(Registry::parse("this is not toml at all {{{").is_err());
        // A rule with no handler cannot be acted on.
        assert!(Registry::parse("name = \"X\"\n[[previewer]]\nextensions = [\"x\"]").is_err());
    }

    #[test]
    fn command_lines_split_on_whitespace_and_honour_quotes() {
        assert_eq!(
            split_arguments("convert %i %o"),
            vec!["convert", "%i", "%o"]
        );
        assert_eq!(
            split_arguments("my-tool --flag 'two words' %i"),
            vec!["my-tool", "--flag", "two words", "%i"]
        );
        assert_eq!(split_arguments("   "), Vec::<String>::new());
    }

    #[test]
    fn a_file_name_can_never_become_another_argument() {
        // The exact attack the splitting order exists to prevent: the command
        // is split first, so a name full of spaces and quotes stays one
        // argument however hostile it is.
        let hostile = Path::new("/tmp/'; rm -rf ~' and more.blend");
        let mut argv = split_arguments("tool --input %i");
        for argument in &mut argv {
            *argument = substitute(argument, hostile, Path::new("/out"), 256);
        }

        assert_eq!(argv.len(), 3, "substitution must not add arguments");
        assert_eq!(argv[2], "/tmp/'; rm -rf ~' and more.blend");
    }

    #[test]
    fn placeholders_are_all_substituted() {
        assert_eq!(
            substitute("%i:%o:%s", Path::new("/in"), Path::new("/out"), 512),
            "/in:/out:512"
        );
    }

    #[test]
    fn a_missing_command_is_reported_rather_than_run() {
        let rule = Rule {
            extensions: vec![],
            mime_types: vec![],
            handler: Handler::Command,
            syntax: None,
            command: None,
            output: Output::Image,
            timeout: None,
        };
        assert!(matches!(
            run(&rule, Path::new("/tmp/x"), 256),
            Err(Error::NoCommand)
        ));
    }

    #[test]
    fn a_command_that_hangs_is_killed() {
        let rule = Rule {
            extensions: vec![],
            mime_types: vec![],
            handler: Handler::Command,
            syntax: None,
            command: Some("sleep 30".to_owned()),
            output: Output::Image,
            timeout: Some(150),
        };

        let started = Instant::now();
        let result = run(&rule, Path::new("/tmp/x"), 256);
        assert!(matches!(result, Err(Error::TimedOut(_))), "got {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the kill must not wait for the command"
        );
    }

    #[test]
    fn a_command_that_writes_nothing_is_reported() {
        let rule = Rule {
            extensions: vec![],
            mime_types: vec![],
            handler: Handler::Command,
            syntax: None,
            command: Some("true".to_owned()),
            output: Output::Image,
            timeout: None,
        };
        assert!(matches!(
            run(&rule, Path::new("/tmp/x"), 256),
            Err(Error::NoOutput)
        ));
    }

    #[test]
    fn a_command_that_fails_reports_its_status() {
        let rule = Rule {
            extensions: vec![],
            mime_types: vec![],
            handler: Handler::Command,
            syntax: None,
            command: Some("false".to_owned()),
            output: Output::Image,
            timeout: None,
        };
        assert!(matches!(
            run(&rule, Path::new("/tmp/x"), 256),
            Err(Error::Failed(_))
        ));
    }

    #[test]
    fn a_command_that_writes_its_output_succeeds_and_cleans_up() {
        let rule = Rule {
            extensions: vec![],
            mime_types: vec![],
            handler: Handler::Command,
            syntax: None,
            // Writes the input's name into the output, which proves both
            // placeholders were substituted and the file was picked up.
            command: Some("cp %i %o".to_owned()),
            output: Output::Text,
            timeout: None,
        };

        let input = std::env::temp_dir().join("peek-test-plugin-input.txt");
        std::fs::write(&input, b"plugin output").expect("write");

        let kept;
        {
            let product = run(&rule, &input, 256).expect("runs");
            assert_eq!(
                std::fs::read_to_string(product.path()).expect("read"),
                "plugin output"
            );
            kept = product.path().to_path_buf();
        }
        assert!(!kept.exists(), "the output is removed with the product");

        let _ = std::fs::remove_file(input);
    }
}
