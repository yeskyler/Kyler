/// Deliberately do not guess, invoke a judge, or silently repair malformed output.
pub fn extract(response: &str) -> Option<String> {
    let lines: Vec<_> = response
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let line = lines.last()?.trim();
    let answer = line.strip_prefix("FINAL ANSWER:")?.trim();
    (!answer.is_empty()
        && lines
            .iter()
            .filter(|line| line.trim().starts_with("FINAL ANSWER:"))
            .count()
            == 1)
        .then(|| answer.to_owned())
}
#[cfg(test)]
mod tests {
    #[test]
    fn accepts_only_a_single_final_answer_line() {
        assert_eq!(
            super::extract("Reasoning\nFINAL ANSWER: 42\n"),
            Some("42".into())
        );
        for text in [
            "42",
            "FINAL ANSWER:",
            "FINAL ANSWER: 42\nMore",
            "FINAL ANSWER: 1\nFINAL ANSWER: 2",
        ] {
            assert_eq!(super::extract(text), None);
        }
    }
}
