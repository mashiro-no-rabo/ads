CORE

the core functionality of this app is to manage a bunch of processes, primarily for local dev stack (for web apps). And provides unified, single-binary monitoring and debugging interface with logs, (OpenTelemetry) traces and spans, and (Prometheus) metrics.

we should accept a simple templated process list,

## Reference

We can potentially use https://docs.rs/minijinja/latest/minijinja/struct.Template.html#method.undeclared_variables for templates and filling in ports, for example use `{{ port.x }}` and then the main app would collect all such variables and reserve the ports and fill in correct value automatically
