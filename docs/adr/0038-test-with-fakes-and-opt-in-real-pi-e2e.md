# Test core behavior with fakes and keep real pi E2E opt-in

Spool v1 tests will emphasize deterministic fakes: temp SQLite databases, temp Git repositories, fake Agent Launchers, fake Delivery Adapter outcomes, and contract tests for the Spool Pi Extension against a test Spool server. Real pi end-to-end tests are opt-in because they require local model authentication and would make normal CI depend on external agent availability.
