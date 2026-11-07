//! Port of GAIA leaderboard scorer.py at Space revision 9f133d7.
//! Source: https://huggingface.co/spaces/gaia-benchmark/leaderboard/blob/9f133d7/scorer.py
pub const VERSION: &str = "gaia-9f133d7-rust-v1";

fn numeric(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}
fn normalized_number(value: &str) -> f64 {
    numeric(&value.replace(['$', '%', ','], "")).unwrap_or(f64::INFINITY)
}
fn normalized_text(value: &str, punctuation: bool) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace() && !(punctuation && c.is_ascii_punctuation()))
        .flat_map(char::to_lowercase)
        .collect()
}
pub fn correct(answer: &str, reference: &str) -> bool {
    if let Some(number) = numeric(reference) {
        return normalized_number(answer) == number;
    }
    if reference.contains([',', ';']) {
        let expected: Vec<_> = reference.split([',', ';']).collect();
        let actual: Vec<_> = answer.split([',', ';']).collect();
        return expected.len() == actual.len()
            && actual.iter().zip(expected).all(|(a, b)| {
                numeric(b).map_or_else(
                    || normalized_text(a, false) == normalized_text(b, false),
                    |n| normalized_number(a) == n,
                )
            });
    }
    normalized_text(answer, true) == normalized_text(reference, true)
}
#[cfg(test)]
mod tests {
    #[test]
    fn matches_gaia_number_list_and_text_rules() {
        for (a, b) in [
            ("$1,200", "1200"),
            ("12%", "12"),
            ("Sea Gull!", "seagull"),
            ("a-b; $2", "a-b,2"),
        ] {
            assert!(super::correct(a, b));
        }
        for (a, b) in [
            ("2,1", "1,2"),
            ("ab,2", "a-b,2"),
            ("12.01", "12"),
            ("42 extra", "42"),
        ] {
            assert!(!super::correct(a, b));
        }
    }
}
