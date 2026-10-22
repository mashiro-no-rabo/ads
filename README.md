# ads

Run a development stack defined in `ads.toml`.

`ads` (or `ads up`) starts services in the foreground. It searches the current
directory and its parents for `ads.toml`; use `ads -c path/to/ads.toml` to choose
a config. With no command or config, it shows usage. See `ads --help` for commands.

While the stack is running, `ads port web` prints the assigned port number for
`web`, and `ads ports` lists all assigned ports.

## Waiting for services

```toml
[services.db]
cmd = "postgres -p {{ports.db}}"
ready = "ready to accept connections"

[services.api]
wait = ["db"]
cmd = "run --db-port {{ports.db}}"
```

`ready` is a regex matched against each stdout/stderr line. The first match marks
the service ready for the stack's lifetime, including across service restarts.

`wait` delays startup until the listed services are ready. Each must be defined
earlier in `ads.toml` and have a `ready` pattern. Waiting services show as
`waiting` in `ads ps`. Dependencies excluded by `ads up svc...` are ignored.

## Conditional services

```toml
[[run]]
if = "reset"
cmd = "reset-db"

[services.worker]
if_cmd = "test -f .env.worker"
cmd = "run-worker"
```

`if` names an option turned on by a `+name` argument: `ads +reset` or
`ads up -d +reset`. Options are fixed for the stack's lifetime. Passing an option
no `if` uses is an error. `ads check +reset` previews which options are on.

`if_cmd` runs before each service start, after any `wait`. Exit code 0 starts the
service. Like `cmd`, it accepts a shell string or an argv array, supports templates,
and uses the service's `cwd` and `env`. Its output goes to the service log.

When both are set, the service starts only if the option is on and `if_cmd`
succeeds. Otherwise, `ads ps` shows `skipped` with the reason. Use `ads start` to
re-evaluate a skipped service. Skipped services satisfy dependents' `wait`.

`[[run]]` steps support both the same way. A skipped step doesn't stop the stack.

## Opening URLs and ports

Define named destinations in `[open]`:

```toml
[services.web]
cmd = "pnpm run dev -- --port {{ports.web}}"

[open]
web = "http://localhost:{{ports.web}}/app"
admin = 3001
docs = "https://example.com/docs"
```

- `ads open web` opens a named destination.
- `ads open` opens the first entry.
- `ads open --all` opens every entry in config-file order.

Ports (1–65535) open as `http://localhost:<port>`. They can be integers or strings,
including `web = "{{ports.web}}"`.

Strings support the same templates as service commands, including `{{env.NAME}}`.
References to `{{ports.name}}` participate in port allocation and require a running
stack. Fixed ports and static URLs work without starting the stack.

Destinations go directly to the macOS `open` command without a shell. Use
`ads check` to validate and preview them without opening anything.
