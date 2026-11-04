# Cantor

**Make UTAU covers in your browser, with or without an AI agent.**

Cantor is a singing synthesizer for [UTAU](https://en.wikipedia.org/wiki/Utau)
voicebanks. It is one program, `cantor`, that runs a piano-roll editor
in your web browser. An AI agent such as Claude can work on the same
song at the same time over MCP, and you watch its edits appear live.

- **Sings any UTAU voicebank** (`oto.ini`, CV and VCV, `prefix.map`,
  `.frq` files) through its own built-in resampler. There's nothing else to
  install, no neural models, and it never downloads anything.
- **Tunes for you.** It adds portamento, overshoot, vibrato, breaths and
  dynamics automatically, and can follow the pitch of an original vocal
  recording.
- **Shows you numbers.** Every render reports how far the pitch is from the
  score and from the reference vocal, phrase by phrase, plus loudness,
  clipping and any lyrics the voicebank can't sing.
- **Reads your files.** It imports MIDI, UST, USTX (OpenUtau) and MusicXML,
  and exports WAV, UST and USTX.
- **Everything is undoable**, including what an agent does. You can save
  checkpoints and go back to them.

> Status: early (0.1). It works end to end, but expect rough edges. Bug
> reports are welcome.

---

## 1. Install

You need [Rust](https://rustup.rs) (via `rustup`) and Git. Cantor builds
with a pinned nightly compiler, and `rustup` downloads it automatically the
first time you build.

```bash
git clone https://github.com/apothecary103/Cantor.git
cd Cantor
cargo install --locked --path crates/cantor
```

This puts a `cantor` command on your PATH (in `~/.cargo/bin`). The first
build takes a few minutes.

<details>
<summary>Using Nix instead</summary>

```bash
nix develop            # or: direnv allow
cargo install --locked --path crates/cantor
```
</details>

Cantor runs on macOS 14+ and Linux. Windows isn't tested yet.

## 2. Add a voicebank

Cantor doesn't ship a voice. Download any UTAU voicebank (many are free;
read each voicebank's terms) and unzip it into one of these folders:

| Where | When |
|---|---|
| `./voicebanks/` | next to where you run `cantor` |
| `~/.local/share/cantor/voicebanks/` | Linux / anywhere |
| `~/Library/Application Support/Cantor/voicebanks/` | macOS |
| `--voicebanks DIR` or `CANTOR_VOICEBANKS=DIR1:DIR2` | anywhere else |

Each voicebank is a folder with an `oto.ini` in it; subfolders (pitch
tiers like `A3/`, `C4/`) are fine. Check what Cantor found:

```bash
cantor voicebanks
```

## 3. Open the editor

```bash
cantor serve              # then open http://127.0.0.1:7878
cantor serve song.mid     # or start with a file: .cantor, .mid, .ust, .ustx, .musicxml
```

Cantor only listens on your own computer (127.0.0.1). Stop it with Ctrl+C.

### Your first song

1. **File ▸ Import file…** (`Ctrl/⌘+I`) a MIDI, UST, USTX or MusicXML
   file, or press `2` and draw notes.
2. Double-click a note to type its lyric (kana or romaji). `Tab` moves to
   the next note. To fill in a whole song, import a `.txt` of the lyrics
   or paste the text with `Ctrl/⌘+V`.
3. Choose the voicebank in the track list on the left.
4. **Tune ▸ Auto pitch**, then **Auto dynamics** and **Insert breaths**.
5. Press `Space` to play. Cantor renders only what changed, so replays are
   fast.
6. Open the **Report** tab on the right to see which phrases are worst.
   Click one to jump to it and loop it.
7. **File ▸ Save** writes a `.cantor` project; **File ▸ Export** writes
   WAV, UST or USTX.

### Following an original vocal

Import a WAV of the original singer with **Import file…** (an isolated vocal works best).
It appears as a **reference**: a blue pitch line behind your notes.

- **Tune ▸ Match reference** makes the pitch follow it.
- `A` switches between your synth and the reference while playing, and
  `Shift+A` plays both.
- `D` shades the places where you are more than 50 cents off.

### Handy keys

| Key | Does |
|---|---|
| `1` `2` `3` `4` | select, draw, knife, pitch pen |
| `Space` / `Home` | play-pause / back to start |
| `Enter` or double-click | edit lyric (`Tab` = next note) |
| `↑` `↓` | transpose a semitone (with `Shift`: an octave) |
| `←` `→` | move by the grid |
| `Ctrl/⌘+Z`, `Ctrl/⌘+Shift+Z` | undo, redo |
| `L` | loop (shift-drag the ruler to set it) |
| `F` | fit to the selection |
| `?` | all shortcuts |

## 4. Let an AI agent make the cover (optional)

Cantor is an [MCP](https://modelcontextprotocol.io) server, so any MCP
client can drive it. With Claude Code:

```bash
claude mcp add cantor -- cantor mcp
```

Then ask, for example: *"Make a cover of song.mid with the lyrics in
lyrics.txt, follow vocal.wav, use voicebank X, and get the pitch within
25 cents of the reference."* The agent reads Cantor's own guide
(`cantor://guide`). It imports, tunes, renders, reads the report and fixes
the worst phrases.

**Watching it live:** keep `cantor serve` running and point the agent at
it instead, with `claude mcp add --transport http cantor http://127.0.0.1:7878/mcp`.
The agent and you then edit the same song. Notes it changes flash in the
editor, the **History** tab tags its edits, and you can undo any of them.
The **Connect** button in the editor shows these commands.

## 5. Command line

Everything the editor does is also available without it:

```bash
cantor cover --midi song.mid --lyrics lyrics.txt --audio vocal.wav \
             --singer <voicebank-id> -o out/     # one-shot tuned cover
cantor import song.ust -o song.cantor
cantor tune song.cantor auto_pitch --params '{"style":"ballad"}'
cantor render song.cantor -o song.wav           # prints the report
cantor export song.cantor ustx -o song.ustx
cantor --help
```

`cover` writes `cover.wav`, `cover.cantor` (open it in the editor to
polish) and `report.json`. Styles are `natural`, `pop`, `ballad`,
`rock` and `robotic`.

## Troubleshooting

- **"No voicebanks found"**: see step 2. Run `cantor voicebanks` to see
  what Cantor sees.
- **A lyric shows in red / "missing alias"**: the voicebank has no sample for
  that lyric. Check the **Report** tab, then change the lyric or set the
  alias in **Properties**.
- **Japanese file names look garbled**: Cantor reads `oto.ini` as UTF-8 or
  Shift-JIS automatically. If a bank still looks wrong, its files may have
  been unzipped with the wrong encoding. Unzip it again with a tool that
  supports Shift-JIS.
- **Port 7878 is busy**: use `cantor serve --port 8000`.
- **Old Cantor (v1) projects** open directly. They are converted on load,
  and Save asks for a new name so the original stays untouched.

## For developers

```bash
nix develop        # pinned toolchain and tools (or use rustup)
cargo xtask ci     # format, clippy, all tests: the gate for every change
cargo xtask serve  # run the editor from source
```

You can work on the web UI without Rust:
`python3 -m http.server -d web`, then open `http://localhost:8000/?mock`.

Start with [docs/design.md](docs/design.md) (architecture, data model, API),
[docs/tuning.md](docs/tuning.md) (UTAU semantics and tuning rules) and
[docs/agent-guide.md](docs/agent-guide.md). Decisions are recorded in
[docs/decisions/](docs/decisions/).

## License

MIT OR Apache-2.0, at your option. Voicebanks are not part of Cantor and
keep their own terms; read them before publishing a cover.
