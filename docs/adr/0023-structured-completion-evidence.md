# Require structured completion evidence

Tasker will track Criterion Status, Validation Status, and Waivers as structured data rather than relying only on Workpad Note Markdown. Transitions to Human Review or Done require every Acceptance Criterion to be satisfied or waived and every Validation Item to be passed or waived, which keeps handoff/completion gates tied to the Task contract while still allowing explicit exceptions.

Waivers may be created by a Review Agent or Operator, not by the Worker Agent executing the Task, so exceptions remain review/repair decisions rather than self-granted bypasses.

Structured Tasker fields are authoritative for gates and scheduling. Workpad Note Markdown can mirror or explain that state, but Tasker will not parse Markdown checkboxes as the source of truth.
