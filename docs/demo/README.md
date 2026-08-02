# Demo recording

`obsctl-rs.cast` is the source of truth: an [asciinema](https://asciinema.org) recording
of a real `obsctl tui` session, 110x58, ~52s, in **asciicast v3** format.

It is consumed two ways:

- **The microsite** plays the cast itself, via the player vendored in `docs/vendor/`.
  Crisp at any size and scrubbable — this is the good one.
- **The README** embeds `obsctl-rs.gif`, because GitHub cannot play a cast inline.

## Regenerating the GIF

`agg` only reads asciicast v2, and the trim below drops the shell preamble so the GIF
opens on obsctl's splash — a blank first frame is what GitHub would otherwise use as the
poster.

```sh
cargo install --locked --git https://github.com/asciinema/agg   # provides `agg`

asciinema convert -f asciicast-v2 --overwrite obsctl-rs.cast /tmp/v2.cast

python3 - <<'EOF'
import json
START = 4.285   # first full paint of the splash, in the v2 timeline
with open("/tmp/v2.cast") as fh:
    header = fh.readline()
    events = [json.loads(l) for l in fh if l.strip()]
kept = [e for e in events if e[0] >= START]
base = kept[0][0]
with open("/tmp/trim.cast", "w") as fh:
    fh.write(header)
    for e in kept:
        fh.write(json.dumps([round(e[0] - base, 6), e[1], e[2]], ensure_ascii=False) + "\n")
EOF

agg --font-size 12 --fps-cap 10 --speed 1.6 --idle-time-limit 1 \
    --last-frame-duration 3 /tmp/trim.cast obsctl-rs.gif
```

Settings are chosen to land near 1.5 MB at 827x991 — that width renders roughly 1:1 in
GitHub's README column, so the text stays sharp without scaling. Raising `--font-size`
or `--fps-cap` grows the file quickly; if you re-record at a smaller terminal size, drop
`--speed` back toward 1.0 so the pacing still reads.
