# `libra blame`

Trace each line of a file to the commit that last introduced it.

## Human Output

Human mode remains:

```text
abc12345 (Author Name     2026-03-30 10:00:00 +0800 1) line content
```

`-L` supports:

- `10`
- `10,20`
- `10,+5`

## JSON Output

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "tracked.txt",
    "revision": "abc123...",
    "lines": [
      {
        "line_number": 1,
        "short_hash": "abc12345",
        "hash": "abc123...",
        "author": "Test User",
        "date": "1711766400",
        "content": "tracked"
      }
    ]
  }
}
```

## Errors

- Invalid revision or missing file: `LBR-CLI-003`
- Invalid `-L` range: `LBR-CLI-002`
- Failed to read the commit or object: `LBR-REPO-002`
