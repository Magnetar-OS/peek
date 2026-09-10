# Name of the application's binary.
name := 'peek'
# The unique ID of the application.
appid := 'com.magnetaros.Peek'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Application's appstream metadata
appdata := appid + '.metainfo.xml'
# Application's desktop entry
desktop := appid + '.desktop'
# Application's icon. The scalable SVG is what modern toolkits pick up; the
# PNGs are rasterised from it at each size so the panel and the icon grid get
# pixel-exact art instead of a downscaled smudge.
icon-dir := 'res' / 'icons' / 'hicolor'
icon-svg := appid + '.svg'
icon-symbolic := appid + '-symbolic.svg'
icon-sizes := '16x16 24x24 32x32 48x48 64x64 128x128 256x256 512x512'
# The thumbnailer's registration with the thumbnail factories.
thumbnailer-entry := appid + '.thumbnailer'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
appdata-dst := base-dir / 'share' / 'metainfo' / appdata
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / desktop
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps' / icon-svg
icon-symbolic-dst := icons-dst / 'symbolic' / 'apps' / icon-symbolic
thumbnailer-bin-dst := base-dir / 'bin' / 'peek-thumbnailer'
thumbnailer-entry-dst := base-dir / 'share' / 'thumbnailers' / thumbnailer-entry
plugins-dst := base-dir / 'share' / 'peek' / 'plugins'
man-dst := base-dir / 'share' / 'man' / 'man1' 

bin-src := cargo-target-dir / 'release' / name
thumbnailer-bin-src := cargo-target-dir / 'release' / 'peek-thumbnailer'

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build --locked --workspace {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features --locked {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Run the tests
test *args:
    cargo test --workspace --locked {{args}}

# Run the application for testing purposes
run *args:
    cargo run --locked {{args}}

# Report what a file decodes to, without opening a window
probe *args:
    cargo run --locked -p peek-engine --example probe -- {{args}}

# Time the path between pressing space and seeing the file.
#
# Release, because a debug build measures the absence of optimisation. The
# numbers are only comparable against another run on the same machine — see
# the example's own documentation.
bench *args:
    cargo run --locked --release -p peek-engine --example bench -- {{args}}

# Installs files into the system
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0755 {{thumbnailer-bin-src}} {{thumbnailer-bin-dst}}
    install -Dm0644 res/{{desktop}} {{desktop-dst}}
    install -Dm0644 res/{{appdata}} {{appdata-dst}}
    install -Dm0644 res/{{thumbnailer-entry}} {{thumbnailer-entry-dst}}
    install -Dm0644 {{icon-dir}}/scalable/apps/{{icon-svg}} {{icon-svg-dst}}
    install -Dm0644 {{icon-dir}}/symbolic/apps/{{icon-symbolic}} {{icon-symbolic-dst}}
    for size in {{icon-sizes}}; do \
        install -Dm0644 {{icon-dir}}/$size/apps/{{appid}}.png \
            {{icons-dst}}/$size/apps/{{appid}}.png; \
    done
    # The directory the plugin loader reads, plus the worked examples. The
    # examples are documentation, not enabled previewers: they are named
    # `.example-*` and the loader only reads `.toml`.
    install -d {{plugins-dst}}
    install -Dm0644 res/man/peek.1 {{man-dst}}/peek.1
    install -Dm0644 res/man/peek-thumbnailer.1 {{man-dst}}/peek-thumbnailer.1
    install -Dm0644 res/plugins/README.md {{plugins-dst}}/README.md
    install -Dm0644 res/plugins/com.magnetaros.Peek.example-command.toml {{plugins-dst}}/com.magnetaros.Peek.example-command.toml.example
    install -Dm0644 res/plugins/com.magnetaros.Peek.example-text.toml {{plugins-dst}}/com.magnetaros.Peek.example-text.toml.example
    # `Open With → Peek` reads the MIME list out of the desktop database, which
    # is a cache and does not notice a new file on its own. Guarded on rootdir:
    # unguarded, the tools create cache files inside a staged package root and
    # those ship in the package and conflict with other packages' copies.
    if [ -z '{{rootdir}}' ]; then \
        update-desktop-database {{base-dir}}/share/applications || true; \
        gtk-update-icon-cache --force {{icons-dst}} || true; \
    fi

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{thumbnailer-bin-dst}} {{desktop-dst}} {{appdata-dst}} {{thumbnailer-entry-dst}} {{icon-svg-dst}} {{icon-symbolic-dst}}
    for size in {{icon-sizes}}; do \
        rm -f {{icons-dst}}/$size/apps/{{appid}}.png; \
    done
    rm -rf {{plugins-dst}}
    rm -f {{man-dst}}/peek.1 {{man-dst}}/peek-thumbnailer.1
    -update-desktop-database {{base-dir}}/share/applications

# Install to the current user rather than the system, for dogfooding.
install-user:
    just rootdir='' prefix={{env('HOME')}}/.local install

# Vendor dependencies locally
vendor:
    #!/usr/bin/env bash
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    echo >> .cargo/config.toml
    echo '[env]' >> .cargo/config.toml
    if [ -n "${SOURCE_DATE_EPOCH}" ]
    then
        source_date="$(date -d "@${SOURCE_DATE_EPOCH}" "+%Y-%m-%d")"
        echo "VERGEN_GIT_COMMIT_DATE = \"${source_date}\"" >> .cargo/config.toml
    fi
    if [ -n "${SOURCE_GIT_HASH}" ]
    then
        echo "VERGEN_GIT_SHA = \"${SOURCE_GIT_HASH}\"" >> .cargo/config.toml
    fi
    tar pcf vendor.tar .cargo vendor
    rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Validate the desktop entry and the AppStream metadata
validate:
    -desktop-file-validate res/{{desktop}}
    -appstreamcli validate --no-net res/{{appdata}}
