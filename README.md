# har-downloader

Browse a `.har` file in a terminal UI, filter the responses, and save them to a folder.

```sh
cargo install --path .        # or: cargo run --release -- file.har
har-downloader file.har       # opens the TUI, saves to ./dist by default
```

> Export the HAR **with content** (Chrome DevTools → Network → right click → "Save all as HAR with content"),
> otherwise response bodies are missing and there is nothing to save.

## TUI keys

| key | action |
|---|---|
| `↑ ↓` / `j k`, PgUp/PgDn, `g`/`G` | move |
| `space` / `a` / `c` | select file / select all visible / clear selection |
| `t` / `d` | filter by type / domain (checkbox popup) |
| `/` | filter by URL (regex or text, live) |
| `s` | filter by status (`200`, `2xx`, `2xx,304`) |
| `z` | minimum size (`10k`, `1.5m`) |
| `b` | show/hide entries without a body |
| `r` | reset filters |
| `l` | layout: `tree` (host/path) → `flat` → `type` (dist/image/...) |
| `o` | output folder |
| `enter` / `w` | save the selected files, or all visible files if none are selected |
| `?` / `q` | help / quit |

## Flags

Flags pre-fill the TUI filters. Add `--list` or `--yes` to skip the TUI:

```sh
har-downloader file.har -t image,font -d example.com --min-size 5k -l    # list only
har-downloader file.har -t script -m 'chunk|vendor' -s 2xx -o out --layout flat -y   # save directly
```

`-t` accepts types (`image script css font json html xml video audio wasm text other`) or extensions (`png`, `js`, ...).
`-d example.com` also matches subdomains such as `cdn.example.com`.
When the same URL appears more than once, it is saved only once. Different URLs that map to the same file name are saved as `name-1.ext`, `name-2.ext`, and so on.
