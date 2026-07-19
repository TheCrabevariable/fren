# fren ⚡
A fast, lightweight TUI file manager written in Rust. Navigate, organize, and open files **with any app** — no bloat.
## Why fren?
- **Open anything** — Open files and directories with any application you choose
- **Fast** — Built in Rust, instant startup, low memory
- **Image previews** — See your images right in the terminal (via Kitty protocol / chafa)
- **Multi-select** — Copy, cut, paste, trash multiple files at once
- **Clipboard** — Built-in clipboard with copy/cut/paste
- **Pinned directories** — Save your favorite locations for quick access
- **Storage mounts** — Automatically detects mounted drives
- **Configurable keybindings** — Every key is rebindable
- **Themes** — Customize colors to your liking
- **Icon modes** — Emoji, Nerd Font, or ASCII icons
## Installation
### AUR (recommended)
```bash
yay -S fren-bin

yay -S fren-git
```
### Manual
```bash
git clone https://github.com/TheCrabevariable/fren
cd fren
makepkg -si
```
## Usage
| Key | Action |
|-----|--------|
| `o` | Open with... |
| `Enter` | Open with program prompt |
| `→` | Enter directory / open file |
| `←` | Go up a directory |
| `c` | Copy selected |
| `x` | Cut selected |
| `v` | Paste |
| `d` | Trash |
| `r` | Rename |
| `n` | New file |
| `f` | New folder |
| `s` | Cycle sort mode |
| `.` | Toggle hidden files |
| `u` | Pin directory |
| `i` | Unpin directory |
| `m` | Go to path |
| `Tab` | Cycle focus (Files / Pinned / Storage / Clipboard) |
| `Space` | Toggle selection |
| `/` | Show help |
| `q` | Quit |
> All keybindings are configurable in `~/.config/fren/config.toml`
## Screenshot
<img width="1893" height="1021" alt="image" src="https://github.com/user-attachments/assets/acc9dc9d-d1c4-433d-9572-613fa0fb2748" />

## Emoji / Icons
If emoji don't display properly (common in Kitty):
```bash
sudo pacman -S noto-fonts-emoji
```
Add to your shell config:
```bash
export FREN_ICON_MODE=emoji
```
Add to your terminal config:
```
symbol_map U+1F300-U+1F9FF Noto Color Emoji
```
For Nerd Font icons, install a Nerd Font and set:
```bash
export FREN_ICON_MODE=nerd
```
## Configuration
- **Keybindings**: `~/.config/fren/config.toml`
- **Theme**: `~/.config/fren/theme.toml`
- **Pinned directories**: `~/.config/fren/pinned.txt`
- **Session (remember last dir)**: `~/.config/fren/session.txt`

See [Wiki](https://github.com/TheCrabevariable/fren/wiki) for full config and theme documentation.

## License
MIT
