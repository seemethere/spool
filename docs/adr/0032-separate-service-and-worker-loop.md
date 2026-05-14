# Separate the Spool Service from the worker loop

Spool v1 will start the HTTP task service with `spool serve` and run agent work with an explicit `spool work --queue <key>` worker loop. A `--once` mode will claim and run at most one Task for debugging; workers are not hidden inside `spool serve` by default, which keeps local automation explicit and easier to stop, inspect, and test.
