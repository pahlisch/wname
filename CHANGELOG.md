# Changelog

## 0.2.0

A rewrite in Rust of the original Python script.

### Changed

- One daemon draws every band, instead of one Python/GTK process per band
  (each about 146 MB of memory). The daemon exits once the last band is gone.
- It follows Hyprland's event socket, and samples positions by reading the IPC
  socket directly. The old version started two `hyprctl` processes per band,
  every 50 ms.
- The band height comes from the font, not a hard-coded 22 px. GTK drew the
  18 px font 24 px tall, so the old margins and the real surface size disagreed.
- Holds bands back while Hyprland animates a window or workspace into place,
  so a band never races ahead of its window.
- `wname -n` names the window it runs in (the terminal whose process started
  it), not whichever window has focus. An agent naming its own terminal no
  longer names the window you are typing in. `--focused` keeps the old
  behaviour, and keybindings still use the focused window.
- New: `--list`, `--clear-all`, `--colors`, `--quit`, `-w/--window ADDRESS`,
  `#rgb` colors, and readable text on custom colors.
- A new band gets a color that no other band is using.
- Bands survive a daemon restart.

### Fixed

- Clicks on the band went to the band, not to the window underneath.
- A long name made the band wider than its window.
- On a second monitor the band could appear on the wrong output. Monitor
  scale and rotation were ignored.
- Floating windows only covered a band when they had focus more recently, so
  a focused tiled window's band was drawn on top of a floating window.
- Any 7-character string starting with `#` was accepted as a color and
  written into the stylesheet as is.
- Stale pidfiles were left behind when a band process died.
