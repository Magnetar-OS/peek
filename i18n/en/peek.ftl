# Desktop entry and AppStream metadata. Expanded at build time by xdgen, so
# these three are what a translated launcher entry is built from.
app-title = Peek
app-comment = Preview files without opening them
app-keywords = preview;quicklook;quick look;view;peek;

## Overlay chrome

open = Open
copy-path = Copy path
fullscreen = Fullscreen
index-sheet = All files
no-file = No file
loading = Loading…
nothing-to-show = Nothing to show

## Settings

settings-appearance = Appearance
settings-content = Content
settings-behaviour = Behaviour
setting-blur = Frosted backdrop
setting-animate = Animate transitions
setting-opacity = Panel opacity
setting-max-fraction = Largest size
setting-line-numbers = Line numbers
setting-autoplay = Play media automatically
setting-loop = Repeat media
setting-follow-selection = Move the file manager's selection
setting-click-away = Click outside to close
autoplay-always = Always
autoplay-video-only = Video only
autoplay-never = Never

## Summaries shown beside the file name

# A pixel or point size, e.g. 1920 × 1080
dimensions = { $width } × { $height }
animated = Animated
page-of = page { $page } of { $pages }
position-of = { $current } of { $total }
line-count = { $count ->
        [one] { $count } line
       *[other] { $count } lines
    }
item-count = { $count ->
        [one] { $count } item
       *[other] { $count } items
    }
archive-summary = { $size }, { item-count }
folder-summary = { $files ->
        [one] { $files } file
       *[other] { $files } files
    }, { $folders ->
        [one] { $folders } folder
       *[other] { $folders } folders
    }

## Books and fonts

truncated-book = Showing the opening pages
glyph-count = { $count ->
        [one] { $count } glyph
       *[other] { $count } glyphs
    }
field-glyphs = Glyphs
field-version = Version
field-faces = Faces
font-variable = Variable
font-monospaced = Monospaced
# Shown at several sizes, set in the font being previewed. A pangram — a
# sentence using every letter — is the convention for a specimen; translators
# should substitute a pangram for their own alphabet rather than translate
# this one literally.
pangram = The quick brown fox jumps over the lazy dog
# The letterforms a specimen is judged on, beside the pangram.
specimen-alphabet = ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789

## Text previews

truncated-file = Showing the first part of a longer file
lossy-file = Some bytes are not valid text and were replaced

## Archives

empty-archive = This archive is empty
locked-entry = locked
more-entries = more entries not listed
uncompressed-total = { $size } uncompressed

## Folders

unreadable-folder = This folder cannot be read
partial-folder = Only part of this folder was counted
contents = Contents
size = Size

## Media

no-codec = No codec on this system can decode this file
stream = Stream
# Elapsed over total, e.g. 1:05 / 3:42
transport-position = { $position } / { $duration }

## MIME type names
#
# $subtype is the format's own name, already uppercased: PNG, FLAC, TAR.

mime-folder = Folder
mime-pdf = PDF document
mime-zip = Zip archive
mime-plain-text = Plain text
mime-binary = Binary file
mime-image = { $subtype } image
mime-video = { $subtype } video
mime-audio = { $subtype } audio
mime-text = { $subtype } text
mime-font = { $subtype } font
mime-file = { $subtype } file

## Why a previewer declined, shown above the metadata card

reason-unreadable = The file could not be read
reason-undecodable = The file could not be decoded
reason-encrypted = This file is encrypted and needs a password
reason-too-large = The image is too large to preview
reason-empty = There is nothing in this file to show
reason-no-preview = The file carries no embedded preview

## Metadata card rows

field-kind = Kind
field-type = Type
field-size = Size
field-modified = Modified
field-permissions = Permissions
field-symlink-to = Symlink to
field-where = Where

# A size in human units followed by the exact byte count
size-with-bytes = { $size } ({ $bytes } bytes)

## Modification times

just-now = just now
minutes-ago = { $count ->
        [one] { $count } minute ago
       *[other] { $count } minutes ago
    }
hours-ago = { $count ->
        [one] { $count } hour ago
       *[other] { $count } hours ago
    }
days-ago = { $count ->
        [one] { $count } day ago
       *[other] { $count } days ago
    }
# An absolute date, already formatted as digits
on-date = { $date } UTC
before-epoch = before 1970
