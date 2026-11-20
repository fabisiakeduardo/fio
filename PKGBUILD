# Maintainer: Eduardo Fabisiak <fabisiakeduardo@users.noreply.github.com>
pkgname=fio
pkgver=0.1.0
pkgrel=1
pkgdesc="Virtual audio patchbay for PipeWire/PulseAudio — Loopback for Linux"
arch=('x86_64')
url="https://github.com/eduardo/fio"
license=('MIT')
depends=(
  'libpulse'
  'webkit2gtk-4.1'
  'gtk3'
)
optdepends=(
  'pipewire-pulse: PulseAudio-compatible audio server (or pulseaudio)'
  'libayatana-appindicator: system tray icon (or libappindicator)'
)
makedepends=('cargo' 'npm' 'git')
# Local-repo source: clones this checkout's committed state, so `makepkg -s`
# works today with no git remote — uncommitted changes are NOT included.
# Once a tagged release exists upstream, swap in
#   "$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz" + real sha256sums.
source=("$pkgname::git+file://$startdir")
sha256sums=('SKIP')

prepare() {
  cd "$srcdir/$pkgname"
  npm ci
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --manifest-path src-tauri/Cargo.toml
}

build() {
  cd "$srcdir/$pkgname"
  # Build the frontend + release binary without running the bundler:
  # the package layout is owned by package(), not tauri-bundler.
  npx tauri build --no-bundle
}

package() {
  cd "$srcdir/$pkgname"

  install -Dm755 "src-tauri/target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"

  install -Dm644 /dev/stdin "$pkgdir/usr/share/applications/$pkgname.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=fio
Comment=$pkgdesc
Exec=$pkgname
Icon=$pkgname
Categories=AudioVideo;Audio;
Terminal=false
StartupWMClass=fio
EOF

  install -Dm644 "src-tauri/icons/32x32.png" \
    "$pkgdir/usr/share/icons/hicolor/32x32/apps/$pkgname.png"
  install -Dm644 "src-tauri/icons/128x128.png" \
    "$pkgdir/usr/share/icons/hicolor/128x128/apps/$pkgname.png"
  install -Dm644 "src-tauri/icons/128x128@2x.png" \
    "$pkgdir/usr/share/icons/hicolor/256x256/apps/$pkgname.png"

  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
