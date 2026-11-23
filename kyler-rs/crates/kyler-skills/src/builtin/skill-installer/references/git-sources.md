# Git sources

- Use an advertised command tool to fetch the selected repository/ref into a
  temporary directory under an authorized workspace root. Pass inputs as separate
  arguments when the tool supports them. End repository options before the clone
  URL and disable recursive submodules. Use existing Git authentication for
  private repositories, keeping credentials out of URLs, logs, and files.
- Locate the selected directory containing SKILL.md. In a repository that holds
  several skills, import only the selected directory. Keep Git metadata out of
  the package, and inspect referenced scripts as text.
- If the command is prohibited, Git is unavailable, or the fetch fails, report
  the cause and use a user-provided local directory if available. A fetch failure
  does not establish that the skill is absent.
