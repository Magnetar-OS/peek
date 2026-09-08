// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org.gnome.NautilusPreviewer2` — the space bar.
//!
//! ## Why this interface
//!
//! QuickLook's defining gesture is pressing space in a file manager, and the
//! only part of that gesture a separate process can implement is the second
//! half. Something has to tell the previewer *which file is selected*, and the
//! only program that knows is the file manager.
//!
//! On Linux exactly one interface does this:
//! `org.gnome.NautilusPreviewer2`. Nautilus calls `ShowFile` when the user
//! presses space, `Close` when the preview should go away, and subscribes to
//! `SelectionEvent` so that arrow keys pressed *inside* the preview move its
//! selection — which then produces another `ShowFile`. That round trip is the
//! whole gesture, and serving this interface is what buys it.
//!
//! GNOME's own previewer, `sushi`, claims the same name. Only one process can
//! own it, so `peek` asks to replace the existing owner and yields it back on
//! exit. Masking `org.gnome.NautilusPreviewer.service` makes the takeover
//! permanent; leaving it alone means sushi comes back next login.
//!
//! ## cosmic-files
//!
//! cosmic-files does not call this interface, or any other. Its preview is
//! internal, its key bindings are compiled in, and it exposes no way to learn
//! what is selected — so the space bar cannot be made to reach an external
//! previewer without a change to cosmic-files itself. Until then `peek` reaches
//! COSMIC through "Open With" and through its own command line, and reaches
//! Nautilus through this interface.
//!
//! ## The signal's argument type
//!
//! sushi's introspection declares `SelectionEvent` as carrying `q` (`u16`), but
//! Nautilus parses the payload with `g_variant_get(parameters, "(u)")` — a
//! `u32`. Emitting `q` produces a GLib critical in Nautilus and a garbage
//! direction. The consumer decides: this emits `u`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use zbus::object_server::SignalEmitter;
use zbus::{interface, zvariant};

/// Well-known name Nautilus talks to.
pub const NAME: &str = "org.gnome.NautilusPreviewer";

/// Object path the interface is served at.
pub const PATH: &str = "/org/gnome/NautilusPreviewer";

/// `GtkDirectionType`, which is what Nautilus reads the direction as.
///
/// Nautilus hands the value straight to its view, so the horizontal pair moves
/// the selection in a grid view and the vertical pair moves it in a list view.
/// Sending the arrow the user actually pressed lets each view do the right
/// thing without the previewer having to know which one is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Direction {
    Up = 2,
    Down = 3,
    Left = 4,
    Right = 5,
}

/// What the file manager asked for.
#[derive(Debug, Clone)]
pub enum Request {
    /// Preview this file.
    Show {
        path: PathBuf,
        /// Set when the file manager wants a second press of space on the same
        /// file to dismiss rather than reload — which is how the gesture
        /// toggles.
        close_if_shown: bool,
    },
    /// Hide the preview.
    Close,
}

/// State the interface exposes as properties, owned by the application.
///
/// Behind a mutex rather than passed by message because D-Bus property reads are
/// synchronous from the caller's point of view: answering them by round-tripping
/// through the application's update loop would make a property read wait on
/// whatever the previewer happened to be decoding.
#[derive(Debug, Default)]
pub struct Shared {
    pub visible: bool,
    /// Wayland/X11 handle of the window that asked for the preview. Reported
    /// back verbatim; `peek` maps a layer surface, which has no parent, so this
    /// is bookkeeping for the caller rather than something acted on.
    pub parent_handle: String,
}

struct Previewer {
    requests: mpsc::UnboundedSender<Request>,
    shared: Arc<Mutex<Shared>>,
}

#[interface(name = "org.gnome.NautilusPreviewer2")]
impl Previewer {
    /// Show a file, named by URI.
    ///
    /// Non-`file://` URIs are declined rather than guessed at: every previewer
    /// in this crate reads from a path, and silently mapping a `smb://` URI to
    /// a GVFS mount point that may not exist would fail later and less clearly.
    async fn show_file(
        &self,
        uri: &str,
        window_handle: &str,
        close_if_already_shown: bool,
        activation_token: &str,
    ) {
        // The activation token is for handing focus to a window being raised.
        // `peek` maps a layer surface with an exclusive keyboard grab, so there
        // is nothing to hand focus to — but the argument is part of the
        // interface and dropping it from the signature would change the wire
        // format.
        let _ = activation_token;

        let Some(path) = path_from_uri(uri) else {
            tracing::warn!(uri, "ignoring a preview request for a non-local URI");
            return;
        };

        if let Ok(mut shared) = self.shared.lock() {
            shared.parent_handle = window_handle.to_owned();
        }

        let _ = self.requests.send(Request::Show {
            path,
            close_if_shown: close_if_already_shown,
        });
    }

    /// Hide the preview.
    async fn close(&self) {
        let _ = self.requests.send(Request::Close);
    }

    /// Handle of the window that requested the current preview.
    #[zbus(property)]
    async fn parent_handle(&self) -> String {
        self.shared
            .lock()
            .map(|shared| shared.parent_handle.clone())
            .unwrap_or_default()
    }

    /// Whether a preview is on screen.
    #[zbus(property)]
    async fn visible(&self) -> bool {
        self.shared.lock().is_ok_and(|shared| shared.visible)
    }

    /// Ask the caller to move its selection.
    ///
    /// Declared here rather than only emitted by hand so that the interface
    /// introspects honestly: a client reading the XML has to be able to see the
    /// signal it is expected to subscribe to.
    #[zbus(signal)]
    async fn selection_event(emitter: &SignalEmitter<'_>, direction: u32) -> zbus::Result<()>;
}

/// A running previewer service.
///
/// Holds the connection open: dropping it releases the bus name, which is how
/// sushi gets the name back when `peek` exits.
pub struct Service {
    connection: zbus::Connection,
    shared: Arc<Mutex<Shared>>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish_non_exhaustive()
    }
}

impl Service {
    /// Record that the preview appeared or disappeared.
    ///
    /// Nautilus watches `Visible` to decide whether a second press of space is a
    /// toggle or a fresh request, so letting it go stale breaks the gesture in a
    /// way that looks like the previewer ignoring input.
    pub fn set_visible(&self, visible: bool) {
        if let Ok(mut shared) = self.shared.lock() {
            if shared.visible == visible {
                return;
            }
            shared.visible = visible;
        }

        // The property is exposed as a changing one, so a client that cached it
        // has to be told. Emitted through the object server rather than by hand
        // so the signal carries the right invalidation payload.
        let connection = self.connection.clone();
        tokio::spawn(async move {
            let Ok(interface) = connection
                .object_server()
                .interface::<_, Previewer>(PATH)
                .await
            else {
                return;
            };
            let previewer = interface.get().await;
            if let Err(error) = previewer.visible_changed(interface.signal_emitter()).await {
                tracing::debug!(%error, "could not announce the visibility change");
            }
        });
    }

    /// Ask the file manager to move its selection.
    ///
    /// Fire-and-forget. The file manager may not be listening — it may not even
    /// be the program that opened this preview — and the previewer's own arrow
    /// navigation does not depend on the answer.
    pub fn move_selection(&self, direction: Direction) {
        let connection = self.connection.clone();
        tokio::spawn(async move {
            let Ok(interface) = connection
                .object_server()
                .interface::<_, Previewer>(PATH)
                .await
            else {
                return;
            };

            if let Err(error) =
                Previewer::selection_event(interface.signal_emitter(), direction as u32).await
            {
                tracing::debug!(%error, "could not signal the selection change");
            }
        });
    }
}

/// Start the previewer service and hand back a request stream.
///
/// Returns `None` when the session bus is unreachable or the name is held by an
/// owner that refuses to yield it. Neither is fatal: `peek` still works from its
/// command line and from "Open With", so a missing service is logged and the
/// application carries on.
pub async fn spawn() -> Option<(Service, mpsc::UnboundedReceiver<Request>)> {
    let (requests, stream) = mpsc::unbounded_channel();
    let shared = Arc::new(Mutex::new(Shared::default()));

    let previewer = Previewer {
        requests,
        shared: Arc::clone(&shared),
    };

    // The object server is set up before the name is requested. A client that
    // sees the name appear will call `ShowFile` immediately, and answering
    // `UnknownObject` at that point loses the very first preview.
    let connection = match zbus::connection::Builder::session()
        .and_then(|builder| builder.serve_at(PATH, previewer))
        .map(zbus::connection::Builder::build)
    {
        Ok(future) => match future.await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "could not serve the previewer interface");
                return None;
            }
        },
        Err(error) => {
            tracing::warn!(%error, "could not reach the session bus");
            return None;
        }
    };

    // `ReplaceExisting` takes the name from sushi if it is running;
    // `AllowReplacement` gives it back to whatever asks next, so uninstalling
    // `peek` does not leave Nautilus without a previewer. `DoNotQueue` turns a
    // refusal into an immediate answer rather than a subscription that might
    // fire hours later, mid-session.
    use zbus::fdo::RequestNameFlags;
    let flags = RequestNameFlags::ReplaceExisting
        | RequestNameFlags::AllowReplacement
        | RequestNameFlags::DoNotQueue;

    match connection.request_name_with_flags(NAME, flags).await {
        Ok(_) => tracing::info!(name = NAME, "serving the file manager preview interface"),
        Err(error) => {
            tracing::warn!(%error, name = NAME, "another previewer holds the name");
            return None;
        }
    }

    Some((Service { connection, shared }, stream))
}

/// Convert a URI to a path this process can read.
///
/// Every previewer in the engine reads from a path, so a URI has to become one
/// or be declined. `file://` is a direct conversion; a remote URI — `smb://`,
/// `sftp://`, `mtp://` — is resolved only if GVFS has *already* mounted it,
/// and declined otherwise.
///
/// That distinction is the whole decision. A previewer is pointed at whatever
/// the cursor is over, so triggering a mount would mean a keypress could open
/// a network connection, prompt for credentials, and block on a server that
/// may not answer — from a gesture the user thinks of as "look at this". So
/// nothing here mounts anything. When the file manager already has the share
/// open, which is the case whenever the user is actually looking at it, the
/// mount exists and the preview works.
#[must_use]
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    // A bare path is accepted too: the command line produces them, and having
    // one function that understands both means callers do not have to guess
    // which they are holding.
    if uri.starts_with('/') {
        return Some(PathBuf::from(uri));
    }

    let parsed = url::Url::parse(uri).ok()?;
    if parsed.scheme() == "file" {
        return parsed.to_file_path().ok();
    }

    mounted_path(uri)
}

/// Where GVFS has mounted a remote URI, if it has.
///
/// GIO owns this mapping — the FUSE daemon's path for a mount is not derivable
/// from the URI, it is bookkeeping only the library holds — so this asks it
/// rather than reconstructing it. `path()` answers only when a local path
/// exists, which is exactly "already mounted"; it never causes one.
fn mounted_path(uri: &str) -> Option<PathBuf> {
    // `path` lives on the extension trait, as every GObject accessor does.
    use gio::prelude::FileExt;

    let path = gio::File::for_uri(uri).path()?;
    // The FUSE path can be stale after a share goes away, and a preview of a
    // path that no longer resolves is a decode failure rather than a decline.
    path.exists().then_some(path)
}

/// Express a path as a `file://` URI.
#[must_use]
pub fn uri_from_path(path: &std::path::Path) -> Option<String> {
    url::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

/// The zvariant crate is required by the `interface` macro's expansion; naming
/// it here keeps the dependency visible to a reader of the manifest.
const _: Option<zvariant::Value<'static>> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_uris_become_paths() {
        assert_eq!(
            path_from_uri("file:///home/user/photo.png"),
            Some(PathBuf::from("/home/user/photo.png"))
        );
    }

    #[test]
    fn percent_encoding_is_decoded() {
        assert_eq!(
            path_from_uri("file:///tmp/two%20words.txt"),
            Some(PathBuf::from("/tmp/two words.txt"))
        );
    }

    #[test]
    fn bare_paths_are_accepted() {
        assert_eq!(
            path_from_uri("/etc/hostname"),
            Some(PathBuf::from("/etc/hostname"))
        );
    }

    #[test]
    fn an_unmounted_remote_uri_is_declined_rather_than_mounted() {
        // Nothing is mounted in a test run, so these resolve to nothing —
        // which is the point: asking must not start a mount, prompt for a
        // password, or block on a server.
        assert_eq!(
            path_from_uri("smb://nonexistent.invalid/share/file.txt"),
            None
        );
        assert_eq!(path_from_uri("sftp://nonexistent.invalid/home/a.txt"), None);
    }

    #[test]
    fn a_uri_with_no_local_form_is_declined() {
        // HTTP is never a mount, so there is no path to find.
        assert_eq!(path_from_uri("https://example.com/a.png"), None);
    }

    #[test]
    fn paths_round_trip_through_a_uri() {
        let path = PathBuf::from("/tmp/two words.txt");
        let uri = uri_from_path(&path).expect("encodes");
        assert_eq!(path_from_uri(&uri), Some(path));
    }

    #[test]
    fn directions_match_gtk_direction_type() {
        // Nautilus casts the payload straight to GtkDirectionType, so these are
        // not free to change.
        assert_eq!(Direction::Up as u32, 2);
        assert_eq!(Direction::Down as u32, 3);
        assert_eq!(Direction::Left as u32, 4);
        assert_eq!(Direction::Right as u32, 5);
    }
}
