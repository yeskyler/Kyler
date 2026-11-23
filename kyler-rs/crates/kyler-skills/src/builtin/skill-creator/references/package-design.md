# Package design

For a request such as “Check this project's release bundle before publishing,”
capture what is specific to the project: expected platform archives, the order
of integrity checks, and the conditions that require stopping. Keep the decision
about publishing subject to the user's authorization.

- Put the release trigger, required inputs, and pass/fail checks in SKILL.md.
- Put platform-specific filenames and exceptions in a reference if they need
  more than a short example.
- Include a script only if an existing command or a short documented procedure
  cannot reliably perform the check. Give it explicit inputs and meaningful
  exit codes, and test a passing input and a failing input.
- Include a format template in assets only when it is reused in output. Skip
  empty directories and general-purpose setup or release documents.
