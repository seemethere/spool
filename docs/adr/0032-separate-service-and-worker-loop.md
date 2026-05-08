# Separate the Tasker Service from the worker loop

Tasker v1 will start the HTTP task service with `tasker serve` and run agent work with an explicit `tasker work --queue <key>` worker loop. A `--once` mode will claim and run at most one Task for debugging; workers are not hidden inside `tasker serve` by default, which keeps local automation explicit and easier to stop, inspect, and test.
