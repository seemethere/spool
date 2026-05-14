---
# Canonical File-backed Task Creation front matter.
# Create with: spool task create --queue <TASK_QUEUE_KEY> --from-file .spool/bootstrap-tasks/<task>.md
# Compatibility path: spool task create --bootstrap --queue <TASK_QUEUE_KEY> --file .spool/bootstrap-tasks/<task>.md
# Accepted priority values: urgent, high, normal, low.
# File-backed Task Creation accepts only backlog or ready as initial Task States; omit state to default to ready.
# Ordinary Spool dogfood Tasks should keep review_required false so they use Agent-Gated Integration.
# Set review_required true only when the Task or Operator explicitly requires Human Review.
title: Replace with a concise Task outcome
priority: normal
state: ready
review_required: false
tags:
  - dogfood
conflict_hints:
  - docs
blocking_task_identifiers: []
acceptance_criteria:
  - Repository behavior or documentation is updated as described by the Task Brief
  - The change uses Spool domain language and avoids unsupported v1 workflow concepts
validation_items:
  - Targeted deterministic checks or documentation review pass
  - Relevant formatting, linting, or test commands pass, or a waiver is recorded
---
# Task Brief

## Context

Replace this section with the context a Worker Agent needs to understand the Task. Use Spool terms such as **Task Queue**, **Task**, **Task State**, **Acceptance Criteria**, **Validation Items**, and **Workpad Note** when those concepts are relevant.

## Requested outcome

Replace this section with the smallest useful outcome for this Task. Keep the scope local to Spool v1 and avoid implying unsupported fields such as due dates, estimates, milestones, custom workflows, external tracker sync, or pull-request-only delivery.

## Workpad Note seed

Replace this section with any initial narrative handoff notes. Structured front matter remains authoritative for gates and scheduling; this Markdown is only the Task Brief used to start the Workpad Note.
