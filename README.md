# lrcsync
lrclib.net client, basically looks in current directory for audio files and pulls the lrc for the song if it exists from lrclib.net. Quick poc atm but I have this because I can't run lrcget on a remote server lol because it has no gui.

## Usage
Simply run:
```
lrcsync
```
in a directory with audio files and for audio files with metadata it'll look them up on lrclib.net and if there is a specific match it'll pull them.

For a more "loose" search you can use the `--search` flag, this will use the search endpoint which can account for different punctuation, misspellings, and case. You can also use `--ignore` to ignore certain properties when searching, for example to ignore the artist name you can do:
```bash
lrcsync --search --ignore artist --tolerance 3.0
```
When search is used as a fallback it will try to match by closest duration. The `--tolerance` flag can be used to set a tolerance in seconds, any results exceeding this threshold will be ignored.

```
Usage: lrcsync [OPTIONS]

Options:
  -u, --lrclib-url <LRCLIB_URL>  [default: https://lrclib.net]
  -a, --hidden
  -f, --force                    overwrite existing lrc files
  -i, --ignore <IGNORE>          ignore the follow properties when searching lrclib by not sending them, comma seperated
  -s, --search                   use searching on lrclib as a fallback
  -t, --tolerance <TOLERANCE>    tolerance in seconds for searching lrclib [default: 5]
  -h, --help                     Print help
  -V, --version                  Print version
```