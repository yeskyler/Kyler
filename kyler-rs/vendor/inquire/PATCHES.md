# Local patch

Based on inquire 0.9.4, upstream commit 3d5b65422a247acc773d767558e34fcb69ab04bf.

The opt-in `compact-options` feature removes the fixed Select gutter when all
option prefixes are empty. Selection remains styled through `selected_option`.
Without a selection style, a separate `Selected:` line preserves plain-terminal
accessibility. All input, filtering, pagination and secret input behavior is unchanged.

Upstream: https://github.com/mikaelmello/inquire
