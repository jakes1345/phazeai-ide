# tally

A small counter service. Clients create named counters, bump them and read
them back over plain HTTP. Everything lives in memory; restart the process and
the counters start again from zero.

## Running

```sh
cargo run --release
```

The server listens on `127.0.0.1:8080` unless told otherwise.

## Configuration

All settings come from the environment:

| Variable          | Default     | Meaning                                   |
|-------------------|-------------|-------------------------------------------|
| `HOST`            | `127.0.0.1` | Address to bind                           |
| `PORT`            | `8080`      | Port to bind                              |
| `TALLY_MAX_NAMES` | `1000`      | Most counters the server will hold        |
| `TALLY_LOG`       | `info`      | `error`, `warn`, `info` or `debug`        |

## API

- `GET /counters` lists every counter as `name value` lines.
- `GET /counters/<name>` returns one counter's value.
- `POST /counters/<name>` adds one and returns the new value.
- `DELETE /counters/<name>` removes the counter.

Names are 1 to 64 characters of letters, digits, `-` and `_`.

## Tests

```sh
cargo test
```
