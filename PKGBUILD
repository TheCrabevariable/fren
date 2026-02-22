# Maintainer: TheCrabeuh <clement.dallasenn@outlook.fr>
pkgname=fren
pkgver=1
pkgrel=1
pkgdesc="a TUI file manager that let you open files and directory with any app"
arch=('x86_64')
url="https://github.com/TheCrabevariable/fren.git"
makedepends=('git' 'rust')
depends=('glibc')
optdepends=(
  "noto-fonts-emoji: for emoji icons"
  "ttf-jetbrains-mono-nerd: for Nerd icon mode"
)
options=()
install=
source=("git+$url")
noextract=()
sha256sums=('SKIP')

build() {
	cd "$srcdir/${pkgname%-VCS}"
	cargo build --release --locked
}

package() {
  cd "$srcdir/$pkgname"

  # Install binary
  install -Dm755 target/release/fren "$pkgdir/usr/bin/fren"

  # Install desktop file
  install -Dm644 assets/fren.desktop \
    "$pkgdir/usr/share/applications/fren.desktop"

  # Install icon (256x256 example)
  install -Dm644 assets/fren.png \
    "$pkgdir/usr/share/icons/hicolor/256x256/apps/fren.png"
}
