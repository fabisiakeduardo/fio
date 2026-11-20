# Maintainer: Eduardo <eduardo@localhost>
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
  'libayatana-appindicator' # tray icon
)
makedepends=('cargo' 'npm')
source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
  cd "$pkgname-$pkgver"
  npm ci
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --manifest-path src-tauri/Cargo.toml
}

build() {
  cd "$pkgname-$pkgver"
  # Build the frontend + release binary without running the bundler:
  # the package layout is owned by package(), not tauri-bundler.
  npx tauri build --no-bundle
}

package() {
  cd "$pkgname-$pkgver"

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
}
