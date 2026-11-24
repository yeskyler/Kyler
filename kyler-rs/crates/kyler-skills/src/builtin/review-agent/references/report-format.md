# Presenting the validated review

Use the task's structured `review` result. Present actionable findings first,
with priority, a changed-file location, the concrete trigger, and its impact.
Preserve the report's confidence and qualifications; do not invent findings or
upgrade severity while summarizing it. Use the user's language for prose.

Report partial coverage and inaccessible or omitted evidence. A stale snapshot
requires revalidation and cannot establish a current clean conclusion. An empty
findings list means no defensible defect was found in the inspected evidence,
not that the implementation is proven correct.

The host owns snapshot metadata, locations, report validation, and any format
repair. Do not forge those fields or claim tests ran. If the delegated task fails,
report that failure rather than converting it into "no findings". Follow-up fixes
are a separate action governed by the user's request.
