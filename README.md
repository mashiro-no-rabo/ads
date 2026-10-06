# ads

Run a development stack defined in `ads.toml`. Use `ads --help` for commands.

## Opening URLs and ports

Define named destinations in an `[open]` section:

```toml
[services.web]
cmd = "npm run dev -- --port {{ports.web}}"

[open]
web = "http://localhost:{{ports.web}}/app"
admin = 3001
docs = "https://example.com/docs"
```

- `ads open web` opens the named destination.
- `ads open` opens the first `[open]` entry in config-file order.
- `ads open --all` opens every destination in config-file order.

Integer ports open as `http://localhost:<port>` and must be between 1 and
65535. A string containing just a port also works, including
`web = "{{ports.web}}"`.

URL strings support the same templates as service commands, including
`{{ports.name}}` and `{{env.NAME}}`. Templated ports use the assignments
of the running stack, so run `ads up` first. Fixed ports and static URLs
can be opened without starting the stack. Port references in `[open]`
participate in allocation when the stack starts.

Destinations are passed directly to the macOS `open` command, without a shell.
The usual config discovery and `ads -c path/to/ads.toml open web` work here too.
`ads check` validates and previews rendered destinations without opening them.
