# vrec

vrec is a web interface to youtube-dl.

## Usage

```
# Prerequisites: youtube-dl, ffmpeg

cat > .env <<END
# Required
ACCESS_KEY=RaNDOmStrINg

# Optional (default: 3000)
PORT=3000

# Optional (default: ./var)
VAR_DIR=/path/to/var_dir
END

cargo build --release

target/release/vrec
```

Then open http://127.0.0.1:3000/download#k=REPLACE_THIS_WITH_ACCESS_KEY .
