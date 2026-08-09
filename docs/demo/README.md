# Demo recording

`obsctl-rs.cast` is the source of truth: an [asciinema](https://asciinema.org) recording
of a real `obsctl tui` session, 112x57, ~65s, in **asciicast v3** format.

It is consumed two ways:

- **The microsite** plays the cast itself, via the player vendored in `docs/vendor/`.
  Crisp at any size and scrubbable — this is the good one, and it is the only showcase
  the site has. There are no dashboard screenshots any more: a still frame goes stale the
  moment a panel is redrawn, and it cannot be checked against the code the way a
  re-recorded cast can.
- **The README** embeds `obsctl-rs.gif`, because GitHub cannot play a cast inline.

## Recording

The cast committed here is **trimmed to the obsctl session itself** — no shell prompt, no
`cargo build` scrollback, no return to the prompt at the end. A raw recording usually has
all three, and the site plays the cast unedited, so a visitor would otherwise sit through
half a minute of build output before anything happened.

Record however you like, then trim to the frame where the splash first paints. The times
below are absolute seconds into the raw recording; find them by dumping the events:

```sh
python3 - <<'EOF'
import json
with open("raw.cast") as fh:
    fh.readline()                      # header
    events = [json.loads(l) for l in fh if l.strip()]
# asciicast v3 stores the gap since the previous event, not an absolute time.
t = 0.0
for e in events:
    t += e[0]
    print(round(t, 3), repr(e[2][:70]))
EOF
```

The splash is the first event that paints a screenful of block-logo color; the tail starts
at the `\x1b[?1049l` that leaves the alternate screen. Then:

```sh
python3 - <<'EOF'
import json
START, END = 28.99, 94.02           # from the dump above
with open("raw.cast") as fh:
    header = fh.readline()
    events = [json.loads(l) for l in fh if l.strip()]
kept, t = [], 0.0
for e in events:
    t += e[0]
    if START <= t <= END:
        # The first kept event opens the timeline, so its gap resets to zero.
        kept.append([0.0 if not kept else e[0], e[1], e[2]])
with open("obsctl-rs.cast", "w") as fh:
    fh.write(header)
    for e in kept:
        fh.write(json.dumps([round(e[0], 6), e[1], e[2]], ensure_ascii=False) + "\n")
EOF
```

## Regenerating the GIF

`agg` only reads asciicast v2, so convert first. No trimming step is needed — the cast is
already trimmed.

```sh
cargo install --locked --git https://github.com/asciinema/agg   # provides `agg`

asciinema convert -f asciicast-v2 --overwrite obsctl-rs.cast /tmp/v2.cast
agg --font-size 11 --fps-cap 8 --speed 2.0 --idle-time-limit 1 \
    --last-frame-duration 3 /tmp/v2.cast obsctl-rs.gif
```

That lands near 2 MB at 772x893, which renders roughly 1:1 in GitHub's README column, so
the text stays sharp without scaling. `--speed 2.0` brings a 65-second session down to
about 33 seconds of playback, which is as long as a README embed can hold attention.

Size is dominated by how much of the screen changes between frames, and the dashboard
animates almost everywhere — gradient chrome, spinners, sparklines, and the audio meters.
Raising `--font-size` or `--fps-cap` therefore grows the file fast; dropping `--fps-cap`
to 6 saves a few hundred kilobytes but visibly stutters the spinners. If you re-record at
a different terminal size, re-check the output width and update the `width` attribute on
the README's `<img>` so it keeps rendering 1:1.
