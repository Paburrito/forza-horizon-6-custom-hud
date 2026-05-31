# Forza Horizon 6 - Custom HUD

A lightweight overlay HUD for Forza Horizon 6, made by blending the visual style of NFSU2's default HUD with Forza Horizon 6's aesthetic. Built with Tauri and Rust.  
Displays a real-time tachometer, speed, gear indicator, and live HP/Torque/Boost gauges by reading Forza's telemetry data output.

![HUD Preview](src-tauri/icons/Square150x150Logo.png)

## Features

- Two HUD styles - Simple (circular tachometer, mix between Forza and NFSU2's tachometer) and Advanced (Race HUD inspired RPM arc)
- Real-time tachometer with automatic rev limiter learning
- Speed display in KM/H or MPH
- Live HP, Torque and Boost gauges
- Gear indicator with shift light
- Per-wheel lockup indicators
- Launch control detection and badge
- Physics-based HUD motion tied to car acceleration and momentum
- Click-through window lock - overlay the game without interference
- Demo mode for setup without the game running
- Remembers every car's rev limiter across sessions
- Multi-monitor support with per-HUD window state

## Download

Grab the latest `.exe` from the [Releases](../../releases) page - no install required, just run it.

## Setup

1. Open Forza Horizon 6
2. Go to **Settings → HUD and Gameplay**
3. Toggle **Data Out** ON
4. Set **Data Out IP Address** to `127.0.0.1`
5. Set **Data Out IP Port** to `5301`
6. Turn off the in-game **Speedometer** (so you're not running two HUDs)
7. Launch the HUD

## Controls & Hotkeys

| Hotkey | Action |
|--------|--------|
| Drag titlebar | Move window |
| `Ctrl+L` | Lock / unlock window & enable click-through |
| `Ctrl+S` | Save current window position |
| `Ctrl+Shift+R` | Restore saved position |
| `Ctrl+R` | Force re-learn rev limiter for current car |
| `Ctrl+Alt+R` | Quick restart the HUD |
| `Ctrl+Alt+0` | Emergency reset window position |

## Building from Source

### Prerequisites

- [Node.js](https://nodejs.org/) v18 or later
- [Rust](https://rustup.rs/) (stable toolchain)
- Tauri CLI: `cargo install tauri-cli`

### Steps

```bash
git clone https://github.com/yourusername/forza-horizon-6-custom-hud.git
cd forza-horizon-6-custom-hud
npm install
npm run tauri build
```

The standalone exe will be at:
```
src-tauri/target/release/Forza-Horizon-6-Custom-HUD-By-Paburrito.exe
```

## Tech Stack

- [Tauri](https://tauri.app/) - native window and system integration
- [Rust](https://www.rust-lang.org/) - UDP telemetry listener and WebSocket bridge
- Vanilla HTML/CSS/JS - HUD rendering via Canvas API

## Credits

Made by **Paburrito** with way too much caffeine, ADHD meds and genuine love for Forza Horizon 6 and an old great that was Need For Speed Underground 2.

## License

MIT - do whatever you want with it, just don't sell it and please give proper credits (link and all) towards this repository.
