# Enforce Task State transitions for normal clients

Spool will reject free-form state mutation for normal clients and instead enforce an explicit v1 transition model, with an operator-only repair override for exceptional fixes. This protects review, validation, and integration gates from accidental agent or script mistakes, while preserving a recovery path when operational data needs to be repaired.

The normal v1 transitions are Backlog → Ready; Ready → In Progress on claim; In Progress → Human Review, Integrating, Done, or Canceled; Human Review → Rework, Integrating, or Canceled; Rework → In Progress, Human Review, Integrating, or Canceled; and Integrating → Done, Rework, or Canceled. Transitions to Human Review, Integrating, or Done require structured acceptance/validation gates to pass unless using a repair override.
